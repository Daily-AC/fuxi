import Foundation
import Speech
@preconcurrency import AVFoundation
import OSLog

/// SFSpeechRecognizer 实时听写——音频吃 AECEngine.shared 的 vpio fan-out。
///
/// 中文识别走 `zh-CN` locale。强制 on-device（`requiresOnDeviceRecognition=true`）—— 按
/// 用户决策（feedback_aigc_no_third_party 同等隐私偏好），不让语音上云。macOS 14+ 支持
/// 中文 on-device，但首次调用系统会下载语言资源。
///
/// 不再自起 AVAudioEngine——挂 AECEngine listener id "main-stt"，用 vpio 原生 Float32 buffer
/// 直接喂 SFSpeech。这样回声消除/AGC/降噪都能吃到，wake earcon 期间用户开口也不会被自激
/// 当成"在"。
///
/// **NOTE**：T2 会用 WhisperKit 整体重写本文件——保留这个 stub-friendly 形态，让 T2 改写
/// 时只动 task/recognizer 部分，audio listener 接入逻辑可以保留。
@MainActor
final class Recognizer {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "speech")

    private let recognizer: SFSpeechRecognizer?
    private let onResult: (String, Bool) -> Void
    /// 麦克风 RMS 电平回调——SwiftUI 悬浮窗波形动画驱动。0~1。
    private let onLevel: (Double) -> Void

    /// audio thread 跟 main 共享的 request slot——audio thread 调 append，main 在 cleanup
    /// 时 swap 掉。SFSpeech 自己保证 append/endAudio 多线程安全。
    private let requestHolder = SttHolder()
    private var task: SFSpeechRecognitionTask?

    /// 静音超时计时器——Apple SFSpeechRecognizer 的 isFinal 触发时机不可靠（默认要 60s
    /// task timeout 才算完）。我们自己做 VAD。
    /// 两套 timeout：
    ///   - leading：用户开口前给的宽限（≥ 第一帧 partial 到达延迟）
    ///   - trailing：开口后停顿多久算说完（自然语流停顿不该被误断）
    private var silenceTimer: DispatchSourceTimer?
    private static let leadingTimeout: TimeInterval = 6.0
    private static let trailingTimeout: TimeInterval = 2.0
    private var lastTranscript: String = ""
    /// AECEngine listener id——进程内最多一个 Recognizer 实例，hardcode 即可。
    nonisolated static let listenerID = "main-stt"

    init(onResult: @escaping (String, Bool) -> Void,
         onLevel: @escaping (Double) -> Void = { _ in }) {
        self.recognizer = SFSpeechRecognizer(locale: Locale(identifier: "zh-CN"))
        self.onResult = onResult
        self.onLevel = onLevel
    }

    func start() {
        guard let recognizer, recognizer.isAvailable else {
            logger.warning("zh-CN recognizer 不可用——可能首次未下载语言资源")
            return
        }
        stop()

        do {
            try AECEngine.shared.start()
        } catch {
            logger.error("AECEngine start 失败: \(error.localizedDescription, privacy: .public)")
            return
        }

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        if recognizer.supportsOnDeviceRecognition {
            request.requiresOnDeviceRecognition = true
        }
        requestHolder.set(request)

        // listener: vpio 原生 Float32 → SFSpeech.append（接受任意 PCM）+ RMS 电平
        let holder = self.requestHolder
        let onLevel = self.onLevel
        AECEngine.shared.addListener(id: Self.listenerID, format: nil) { buffer in
            holder.append(buffer)
            // RMS 在 audio thread 算，hop 一次 main actor 推 callback——SwiftUI @Published
            // 必须 main actor 写，per-frame ~10-20ms 的 hop 频率主线程吃得住（实测无掉帧）。
            let level = computeRMSLevel(buffer)
            Task { @MainActor in onLevel(level) }
        }

        lastTranscript = ""
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            // SFSpeech 在内部 queue 调——hop 到 main actor 改 state。
            guard let self else { return }
            Task { @MainActor in
                if let result {
                    let text = result.bestTranscription.formattedString
                    let changed = text != self.lastTranscript
                    self.lastTranscript = text
                    self.onResult(text, result.isFinal)
                    if result.isFinal {
                        self.cleanup()
                    } else if changed {
                        // 收到非空 partial → 用户已经开口 → 切到 trailing timeout（说完停顿即断）
                        self.armTimer(timeout: Self.trailingTimeout, kind: "trailing")
                    }
                }
                if error != nil {
                    self.cleanup()
                }
            }
        }
        // 进 listening 装"开口前"宽限 timer——给 SFSpeech 启动 + 用户酝酿语句的时间
        armTimer(timeout: Self.leadingTimeout, kind: "leading")
    }

    /// 装/重置静音计时器：超时则 endAudio() → SDK 走 final 路径 → AppState 触发 sendToXuannv。
    /// `kind` 仅用于日志区分 leading（开口前）/ trailing（说完停顿）。
    private func armTimer(timeout: TimeInterval, kind: String) {
        silenceTimer?.cancel()
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + timeout)
        let holder = self.requestHolder
        let logger = self.logger
        let lastTranscriptSnapshot = self.lastTranscript
        t.setEventHandler { [weak self] in
            // DispatchSourceTimer 的 handler 在我们指定的 .main queue 上跑——但 Swift 6 不
            // 信任 queue 标签，要么 hop 一下要么手动声明。直接 dispatch 到 MainActor。
            Task { @MainActor in
                guard let self else { return }
                if self.requestHolder.hasRequest {
                    holder.endAudio()
                    logger.info("\(kind, privacy: .public) timeout → endAudio (transcript=\(lastTranscriptSnapshot, privacy: .public))")
                } else {
                    self.cleanup()
                }
            }
        }
        t.resume()
        silenceTimer = t
    }

    func stop() {
        requestHolder.endAudio()
        cleanup()
        onLevel(0)
    }

    private func cleanup() {
        silenceTimer?.cancel()
        silenceTimer = nil
        AECEngine.shared.removeListener(id: Self.listenerID)
        task?.cancel()
        task = nil
        requestHolder.set(nil)
    }
}

/// 从 PCM buffer 算 RMS → log 归一化到 0~1。静音 ~0.0；正常语音 0.2~0.6；大声 0.8+。
/// nonisolated，audio thread 上跑。
private func computeRMSLevel(_ buffer: AVAudioPCMBuffer) -> Double {
    guard let channelData = buffer.floatChannelData?[0] else { return 0 }
    let frameCount = Int(buffer.frameLength)
    guard frameCount > 0 else { return 0 }
    var sum: Float = 0
    for i in 0..<frameCount {
        let s = channelData[i]
        sum += s * s
    }
    let rms = sqrt(sum / Float(frameCount))
    let db = 20 * log10(max(rms, 1e-7))
    return max(0, min(1, (Double(db) + 50) / 40))
}

/// audio thread 跟 main actor 共享的 SFSpeechRequest 槽位——append/endAudio 文档说
/// SFSpeech 自己保证多线程安全，holder 只负责 swap 原子（NSLock + raw 指针）。
private final class SttHolder: @unchecked Sendable {
    private let lock = NSLock()
    private var request: SFSpeechAudioBufferRecognitionRequest?

    func set(_ req: SFSpeechAudioBufferRecognitionRequest?) {
        lock.lock(); defer { lock.unlock() }
        request = req
    }

    var hasRequest: Bool {
        lock.lock(); defer { lock.unlock() }
        return request != nil
    }

    func append(_ buffer: AVAudioPCMBuffer) {
        lock.lock()
        let req = request
        lock.unlock()
        req?.append(buffer)
    }

    func endAudio() {
        lock.lock()
        let req = request
        lock.unlock()
        req?.endAudio()
    }
}
