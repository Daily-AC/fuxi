import Foundation
import Combine
import OSLog

/// 整个 App 的中心状态——@MainActor 保证 UI 同步。
///
/// 状态机：
///   idle → listening (热键/唤醒) → sending (用户说完) → waiting (HTTP 200)
///        → speaking (收 XuannvVoiceLine) → idle (TTS 念完)
///
/// 注意：WS 连接是常驻的（App 启动就连），XuannvVoiceLine 事件可能在 idle 来（玄女
/// 自驱发话），也要播。所以"speaking"不是只能从 waiting 进入——任何状态都可被打断进
/// speaking（除了 listening 时——那时用户在说话，玄女不打断）。
@MainActor
final class AppState: ObservableObject {
    /// 单例——SwiftUI 的 @StateObject 不方便从 AppDelegate 拿引用，singleton 最直接。
    /// @MainActor class 的 static let 默认 MainActor 隔离；所有访问点（SwiftUI App
    /// body / AppDelegate 的 Task { @MainActor in ... }）都从 MainActor 进，安全。
    static let shared = AppState()

    enum VoicePhase: String {
        case idle
        case listening
        case sending
        case waiting
        case speaking
    }

    /// 唤醒链路当前在哪条腿：远端 wake-server / 本机 SFSpeech 兜底 / 不开（hotkey 模式或未配 token）
    enum WakeMode: String {
        case remote
        case fallback
        case disabled
    }

    @Published var phase: VoicePhase = .idle
    @Published var lastTranscript: String = ""
    @Published var lastVoiceLine: String = ""
    @Published var connectionStatus: String = "disconnected"
    @Published var settings = Settings.load()
    @Published var wakeMode: WakeMode = .disabled

    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "state")

    // 子组件——延迟初始化，权限批准后才 spin up。
    var recognizer: Recognizer?
    var synthesizer: Synthesizer?
    var hotkey: HotkeyMonitor?
    var fuxiClient: FuxiClient?
    var wakeClient: RemoteWakeClient?
    var wakeFallback: LocalWakeFallback?

    // fallback 期间每 60s 试一次主连接复活。@Sendable Timer 包裹见 setupWake。
    private var fallbackProbeTimer: Timer?

    var menuBarIconName: String {
        switch phase {
        case .idle:
            // mic.slash 表示 fallback 模式——主连断了用户得知道。
            return wakeMode == .fallback ? "mic.slash" : "mic"
        case .listening: return "mic.fill"
        case .sending, .waiting: return "ellipsis.bubble"
        case .speaking: return "speaker.wave.2.fill"
        }
    }

    /// App 启动钩子——AppDelegate 在 applicationDidFinishLaunching 调一次。
    func bootstrap() {
        synthesizer = Synthesizer()
        recognizer = Recognizer { [weak self] transcript, isFinal in
            Task { @MainActor in
                self?.handleTranscript(transcript, isFinal: isFinal)
            }
        }
        hotkey = HotkeyMonitor { [weak self] in
            Task { @MainActor in
                self?.toggleListening()
            }
        }
        hotkey?.install(combo: settings.hotkey)

        fuxiClient = FuxiClient(settings: settings) { [weak self] event in
            Task { @MainActor in
                self?.handleConvEvent(event)
            }
        } statusHandler: { [weak self] status in
            Task { @MainActor in
                self?.connectionStatus = status
            }
        }
        fuxiClient?.connect()

        setupWake()
    }

    /// 起 wake 链路。triggerMode == .hotkey 不需要 wake；其它两种先 connect remote，
    /// fallbackRequested 时切 LocalWakeFallback；recovered 切回。
    private func setupWake() {
        guard settings.triggerMode != .hotkey else {
            wakeMode = .disabled
            return
        }
        guard !settings.wakeToken.isEmpty,
              let url = URL(string: settings.wakeServerURL)
        else {
            // 没填 token / URL 不合法——直接走 fallback 模式（用户至少有兜底）。
            logger.warning("wake token / URL 缺失，直接进 fallback")
            wakeFallback = LocalWakeFallback(keywords: settings.wakeKeywords) { [weak self] in
                Task { @MainActor in self?.startListening() }
            }
            wakeMode = .fallback
            wakeFallback?.start()
            return
        }
        let client = RemoteWakeClient(serverURL: url, bearer: settings.wakeToken) { [weak self] event in
            Task { @MainActor in self?.handleWakeEvent(event) }
        }
        let fb = LocalWakeFallback(keywords: settings.wakeKeywords) { [weak self] in
            Task { @MainActor in self?.startListening() }
        }
        wakeClient = client
        wakeFallback = fb
        wakeMode = .remote
        client.connect()
    }

    private func handleWakeEvent(_ event: WakeClientEvent) {
        switch event {
        case .wakeDetected:
            startListening()
        case .fallbackRequested:
            switchToFallback()
        case .reconnected:
            switchToRemote()
        }
    }

    private func switchToFallback() {
        guard wakeMode != .fallback else { return }
        logger.info("wake 切 fallback")
        wakeMode = .fallback
        wakeFallback?.start()
        // 60s 一次试主连接复活——RemoteWakeClient 内部本身也在重试，这里二级保险，主要服务于
        // 长时间断线后用户改了 wakeToken 想立即试通的场景。
        fallbackProbeTimer?.invalidate()
        let timer = Timer(timeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.wakeClient?.reconnect() }
        }
        RunLoop.main.add(timer, forMode: .common)
        fallbackProbeTimer = timer
    }

    /// 设置变更后重置 wake 链路——用户改 wakeServerURL / wakeToken / triggerMode 都走这。
    func reloadWake() {
        wakeClient?.stop()
        wakeFallback?.stop()
        fallbackProbeTimer?.invalidate()
        fallbackProbeTimer = nil
        wakeClient = nil
        wakeFallback = nil
        wakeMode = .disabled
        setupWake()
    }

    private func switchToRemote() {
        guard wakeMode != .remote else { return }
        logger.info("wake 切回 remote")
        wakeMode = .remote
        wakeFallback?.stop()
        fallbackProbeTimer?.invalidate()
        fallbackProbeTimer = nil
    }

    func toggleListening() {
        switch phase {
        case .idle:
            startListening()
        case .listening:
            stopListening(commit: true)
        default:
            // 别的状态下按热键——视为取消（不发任何东西）
            cancelToIdle()
        }
    }

    func startListening() {
        guard phase == .idle else { return }
        // 释放麦给主 Recognizer——fallback 自身在命中 wake 后已 stop，但 hotkey/外部触发路径
        // 这里也兜一次。
        wakeFallback?.stop()
        phase = .listening
        lastTranscript = ""
        recognizer?.start()
    }

    func stopListening(commit: Bool) {
        guard phase == .listening else { return }
        recognizer?.stop()
        if commit, !lastTranscript.isEmpty {
            sendToXuannv(lastTranscript)
        } else {
            phase = .idle
        }
        // 主 Recognizer 释放麦后，如果还在 fallback 模式就把兜底起回来——否则下次"玄女"喊不出。
        if wakeMode == .fallback {
            wakeFallback?.start()
        }
    }

    private func handleTranscript(_ text: String, isFinal: Bool) {
        guard phase == .listening else { return }
        lastTranscript = text
        if isFinal {
            stopListening(commit: true)
        }
    }

    private func sendToXuannv(_ text: String) {
        // 客户端打 [语音] 前缀——玄女据此判断走语音回应路径（详见 roles/xuannv/instructions/tool-map.md
        // §"语音模式 · Jarvis"）。
        let payload = "[语音] " + text
        phase = .sending
        Task {
            do {
                try await fuxiClient?.sendIntervene(text: payload)
                await MainActor.run { self.phase = .waiting }
            } catch {
                logger.error("intervene 失败: \(error.localizedDescription)")
                await MainActor.run { self.phase = .idle }
            }
        }
    }

    /// WS conv 来事件——只关心 XuannvVoiceLine。其他类型让 PWA / firehose 看，App 不响应。
    private func handleConvEvent(_ event: ConvEvent) {
        guard case let .voiceLine(text) = event else { return }
        lastVoiceLine = text
        // listening 时不打断用户——玄女这条 voice line 排队等用户说完；
        // 简化版：直接 drop，玄女文字仍在 IM 看得到。后续 P1 加排队。
        if phase == .listening { return }
        speak(text)
    }

    private func speak(_ text: String) {
        phase = .speaking
        synthesizer?.speak(text) { [weak self] in
            Task { @MainActor in
                self?.phase = .idle
            }
        }
    }

    func cancelToIdle() {
        recognizer?.stop()
        synthesizer?.stop()
        phase = .idle
    }
}
