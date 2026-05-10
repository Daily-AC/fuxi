import AVFoundation
import OSLog

/// AVSpeechSynthesizer 中文 TTS。
///
/// 默认行为（`voiceIdentifier` 空 / 无效）：扫 `AVSpeechSynthesisVoice.speechVoices()`
/// 选 `language.hasPrefix("zh-CN")` 里 `quality.rawValue` 最大的——premium > enhanced
/// > default。系统自带 compact Tingting 音色机械感重，premium「Lingxiu / Yu-shu」等更
/// 像真人；用户在系统设置 → 辅助功能 → 朗读 → 系统语音 下载 premium 包后会自动被选中。
///
/// 用户在 jarvis 设置面板里手动选了具体 voice identifier 时，honor 用户选择不再自动选。
final class Synthesizer: NSObject, AVSpeechSynthesizerDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "tts")
    private let synth = AVSpeechSynthesizer()
    private var onFinish: (() -> Void)?

    override init() {
        super.init()
        synth.delegate = self
    }

    /// `voiceIdentifier`：传 nil / 空 / 无效 都会触发"自动选最高质量 zh-CN"兜底。
    /// `rate`：传 nil 走 `AVSpeechUtteranceDefaultSpeechRate`；建议 0.55（提速 10%）。
    func speak(_ text: String, voiceIdentifier: String? = nil, rate: Float? = nil, completion: @escaping () -> Void) {
        // 队列中已有发言时——抢占（玄女连续两条语音，后到覆盖前）。
        if synth.isSpeaking {
            synth.stopSpeaking(at: .immediate)
        }
        onFinish = completion

        let utt = AVSpeechUtterance(string: text)
        utt.voice = resolveVoice(identifier: voiceIdentifier)
        utt.rate = rate ?? AVSpeechUtteranceDefaultSpeechRate
        utt.pitchMultiplier = 1.0
        synth.speak(utt)
    }

    /// 优先级：用户填的合法 identifier > 自动选最高质量 zh-CN > language=zh-CN 兜底。
    private func resolveVoice(identifier: String?) -> AVSpeechSynthesisVoice? {
        if let id = identifier?.trimmingCharacters(in: .whitespaces), !id.isEmpty,
           let v = AVSpeechSynthesisVoice(identifier: id) {
            logger.debug("tts voice user-picked: \(v.identifier, privacy: .public) quality=\(v.quality.rawValue)")
            return v
        }
        if let best = Self.bestZhCNVoice() {
            logger.debug("tts voice auto-picked: \(best.identifier, privacy: .public) name=\(best.name, privacy: .public) quality=\(best.quality.rawValue)")
            return best
        }
        // 系统连 default zh-CN 都没装的情况几乎不发生——兜底防 crash。
        return AVSpeechSynthesisVoice(language: "zh-CN")
    }

    /// 系统已装的 zh-CN voice 里 quality 最高的（premium=3 > enhanced=2 > default=1）。
    /// 同 quality 多个时取 `speechVoices()` 顺序里第一个（系统排序通常 Tingting/Lili 优先）。
    static func bestZhCNVoice() -> AVSpeechSynthesisVoice? {
        AVSpeechSynthesisVoice.speechVoices()
            .filter { $0.language.hasPrefix("zh-CN") }
            .max { a, b in a.quality.rawValue < b.quality.rawValue }
    }

    /// 设置面板 picker 数据源——按 quality 降序，premium 在前最显眼。
    static func availableZhCNVoices() -> [AVSpeechSynthesisVoice] {
        AVSpeechSynthesisVoice.speechVoices()
            .filter { $0.language.hasPrefix("zh-CN") }
            .sorted { a, b in
                if a.quality.rawValue != b.quality.rawValue {
                    return a.quality.rawValue > b.quality.rawValue
                }
                return a.name < b.name
            }
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
