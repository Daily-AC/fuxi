import Foundation
import Speech
import AVFoundation
import OSLog

/// SFSpeechRecognizer + AVAudioEngine 实时听写。
///
/// 中文识别走 `zh-CN` locale。强制 on-device（`requiresOnDeviceRecognition=true`）—— 按
/// 用户决策（feedback_aigc_no_third_party 同等隐私偏好），不让语音上云。macOS 14+ 支持
/// 中文 on-device，但首次调用系统会下载语言资源。
///
/// 状态：start() 开始持续录音 + 转写；callback 每段中间/最终结果都回调。stop() 停。
/// end-of-speech 由 SFSpeechRecognitionResult.isFinal 自动触发。
final class Recognizer {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "speech")

    private let recognizer: SFSpeechRecognizer?
    private let onResult: (String, Bool) -> Void
    /// 麦克风 RMS 电平回调——SwiftUI 悬浮窗波形动画驱动。0~1。
    private let onLevel: (Double) -> Void

    private var engine: AVAudioEngine?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?

    /// 静音超时计时器——Apple SFSpeechRecognizer 的 isFinal 触发时机不可靠（默认要 60s
    /// task timeout 才算完）。我们自己做 VAD：每次拿到新 partial 重置计时；超时则
    /// endAudio + cleanup 让 result 走 final 路径。
    private var silenceTimer: DispatchSourceTimer?
    private static let silenceTimeout: TimeInterval = 1.5
    private var lastTranscript: String = ""

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

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        if recognizer.supportsOnDeviceRecognition {
            request.requiresOnDeviceRecognition = true
        }
        self.request = request

        let engine = AVAudioEngine()
        self.engine = engine
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            request.append(buffer)
            self?.publishLevel(from: buffer)
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            logger.error("audio engine start 失败: \(error.localizedDescription)")
            self.cleanup()
            return
        }

        lastTranscript = ""
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            guard let self else { return }
            if let result {
                let text = result.bestTranscription.formattedString
                let changed = text != self.lastTranscript
                self.lastTranscript = text
                self.onResult(text, result.isFinal)
                if result.isFinal {
                    self.cleanup()
                } else if changed {
                    // 文本变了 → 还在说话 → 重置静音 timer
                    self.armSilenceTimer()
                }
            }
            if error != nil {
                self.cleanup()
            }
        }
        // 进 listening 立即装一次 timer——用户开口前 1.5s 都没声音也要兜底退出
        armSilenceTimer()
    }

    /// 装/重置静音计时器：1.5s 内没新 partial 就 endAudio() → SDK 走 final 路径
    /// → onResult(isFinal=true) → AppState 触发 sendToXuannv。
    private func armSilenceTimer() {
        silenceTimer?.cancel()
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + Self.silenceTimeout)
        t.setEventHandler { [weak self] in
            guard let self else { return }
            // endAudio 让 SFSpeechRecognizer 当 final——它会再回调一次 onResult(isFinal=true)
            // 走自然清理路径，不直接 cleanup（避免没有 final 触发 sendToXuannv 上层）。
            if let req = self.request {
                req.endAudio()
                self.logger.info("silence timeout → endAudio (transcript=\(self.lastTranscript, privacy: .public))")
            } else {
                self.cleanup()
            }
        }
        t.resume()
        silenceTimer = t
    }

    func stop() {
        request?.endAudio()
        cleanup()
        onLevel(0)
    }

    /// 从 PCM buffer 算 RMS → log 归一化到 0~1 推回 callback。
    /// 静音 ~0.0；正常语音 0.2~0.6；大声 0.8+。波形动画驱动用。
    private func publishLevel(from buffer: AVAudioPCMBuffer) {
        guard let channelData = buffer.floatChannelData?[0] else { return }
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return }
        var sum: Float = 0
        for i in 0..<frameCount {
            let s = channelData[i]
            sum += s * s
        }
        let rms = sqrt(sum / Float(frameCount))
        // log 化让中等音量在中段（人耳感受非线性）。dBFS = 20*log10(rms)。
        // -50 dBFS（接近静音）→ 0；-10 dBFS（响亮）→ 1。
        let db = 20 * log10(max(rms, 1e-7))
        let normalized = max(0, min(1, (Double(db) + 50) / 40))
        onLevel(normalized)
    }

    private func cleanup() {
        silenceTimer?.cancel()
        silenceTimer = nil
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        task?.cancel()
        task = nil
        request = nil
    }
}
