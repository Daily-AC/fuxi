import AVFoundation
import OSLog

/// AVSpeechSynthesizer 中文 TTS。
///
/// 默认普通话女声（`com.apple.voice.compact.zh-CN.Tingting`）；macOS 14+ 自带。用户
/// 想换男声 / 高质量音色（如「Lili 增强版」），系统设置 → 辅助功能 → 朗读 → 系统语音
/// 单独下载，然后在 jarvis 设置里填 voice identifier。
final class Synthesizer: NSObject, AVSpeechSynthesizerDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "tts")
    private let synth = AVSpeechSynthesizer()
    private var onFinish: (() -> Void)?

    override init() {
        super.init()
        synth.delegate = self
    }

    func speak(_ text: String, voiceIdentifier: String? = nil, completion: @escaping () -> Void) {
        // 队列中已有发言时——抢占（玄女连续两条语音，后到覆盖前）。
        if synth.isSpeaking {
            synth.stopSpeaking(at: .immediate)
        }
        onFinish = completion

        let utt = AVSpeechUtterance(string: text)
        if let id = voiceIdentifier, let v = AVSpeechSynthesisVoice(identifier: id) {
            utt.voice = v
        } else {
            utt.voice = AVSpeechSynthesisVoice(language: "zh-CN")
        }
        utt.rate = AVSpeechUtteranceDefaultSpeechRate
        utt.pitchMultiplier = 1.0
        synth.speak(utt)
    }

    func stop() {
        synth.stopSpeaking(at: .immediate)
        onFinish = nil
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        let cb = onFinish
        onFinish = nil
        cb?()
    }

    func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        let cb = onFinish
        onFinish = nil
        cb?()
    }
}
