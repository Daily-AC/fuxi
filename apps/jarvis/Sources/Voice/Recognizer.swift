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
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "speech")

    private let recognizer: SFSpeechRecognizer?
    private let onResult: (String, Bool) -> Void

    private var engine: AVAudioEngine?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?

    init(onResult: @escaping (String, Bool) -> Void) {
        self.recognizer = SFSpeechRecognizer(locale: Locale(identifier: "zh-CN"))
        self.onResult = onResult
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
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            request.append(buffer)
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            logger.error("audio engine start 失败: \(error.localizedDescription)")
            self.cleanup()
            return
        }

        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            guard let self else { return }
            if let result {
                let text = result.bestTranscription.formattedString
                self.onResult(text, result.isFinal)
                if result.isFinal { self.cleanup() }
            }
            if error != nil {
                self.cleanup()
            }
        }
    }

    func stop() {
        request?.endAudio()
        cleanup()
    }

    private func cleanup() {
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        task?.cancel()
        task = nil
        request = nil
    }
}
