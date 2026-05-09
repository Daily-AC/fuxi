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
    /// `nonisolated(unsafe)` 是因为 static let 需要在任何 actor 之外初始化；实际访问
    /// 全部 @MainActor 隔离。
    nonisolated(unsafe) static let shared = AppState()

    enum VoicePhase: String {
        case idle
        case listening
        case sending
        case waiting
        case speaking
    }

    @Published var phase: VoicePhase = .idle
    @Published var lastTranscript: String = ""
    @Published var lastVoiceLine: String = ""
    @Published var connectionStatus: String = "disconnected"
    @Published var settings = Settings.load()

    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "state")

    // 子组件——延迟初始化，权限批准后才 spin up。
    var recognizer: Recognizer?
    var synthesizer: Synthesizer?
    var hotkey: HotkeyMonitor?
    var fuxiClient: FuxiClient?

    var menuBarIconName: String {
        switch phase {
        case .idle: return "mic"
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
