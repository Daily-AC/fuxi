import Foundation
@preconcurrency import AVFoundation
import OSLog
import RealTimeCutVADLibrary

// 语音活动检测（VAD）——Silero v5 ONNX，包了 ObjC 的 VADWrapper（RealTimeCutVADLibrary）。
//
// 替原本 Recognizer 里手写的 leading 6s + trailing 2s 计时器：
//   - 自然停顿 ~1s 即收（DNN 比固定 timer 准多了）
//   - 短促"嗯"/咳嗽不会触发 onSpeechStart（min speech 200ms）
//   - leading 兜底：6s 没 onSpeechStart → AppState 自己 timer 关
//
// 接 AECEngine 的 16kHz mono Float32 listener id "vad"——跟 Recognizer 共享同一份 vpio
// 输入。VADWrapper 内部以 ~30ms（480 samples@16k）chunk 喂 Silero，但我们这里不强切：
// processAudioData 接受任意长度 buffer，库内部 ring 切。
//
// 鸣谢 https://github.com/helloooideeeeea/RealTimeCutVADLibrary（Silero ONNX + WebRTC APM
// 包成 Swift Package；MIT license）。

@MainActor
final class VAD: NSObject {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "vad")

    private let onSpeechStart: () -> Void
    private let onSpeechEnd: () -> Void

    /// VADWrapper 由 ObjC 实现 + `instancetype` init 在 Swift 桥成 optional——理论上不会 fail，
    /// 实测上确实非 nil；用 ! 直接拿出来比每次链式 `?` 干净。
    /// audio thread 上调它的 processAudioDataWithBuffer:count: 是 ObjC nonisolated 方法，库内自加锁。
    private let wrapper: VADWrapper = VADWrapper()!
    nonisolated static let listenerID = "vad"

    /// 标记当前有没有挂 listener——防止重复 start。
    private var running = false

    init(onSpeechStart: @escaping () -> Void,
         onSpeechEnd: @escaping () -> Void) {
        self.onSpeechStart = onSpeechStart
        self.onSpeechEnd = onSpeechEnd
        super.init()
        wrapper.delegate = self
        wrapper.setSileroModel(.v5)
        wrapper.setSamplerate(.SAMPLERATE_16)
        // 阈值：start 0.5（默认 0.7 偏严，安静室内说话有时摸不到），end 0.35（比 start 低更易判定停顿）；
        // start 需连续 8 帧（8*32ms ≈ 256ms）超阈→真说话；
        // end 需连续 24 帧（≈ 768ms）低于阈→真停顿，自然语流停顿不被切。
        wrapper.setThresholdWithVadStartDetectionProbability(
            0.5,
            vadEndDetectionProbability: 0.35,
            voiceStartVadTrueRatio: 0.8,
            voiceEndVadFalseRatio: 0.95,
            voiceStartFrameCount: 8,
            voiceEndFrameCount: 24
        )
    }

    /// 起 VAD：注册 AECEngine 16kHz Float32 listener。重入幂等——重复 start = noop。
    func start() {
        guard !running else { return }
        guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                         sampleRate: 16_000,
                                         channels: 1,
                                         interleaved: false) else {
            logger.error("VAD 16kHz format 构造失败")
            return
        }
        running = true
        // wrapper 是 NSObject，本身在 Swift 6 严格模式下默认非 Sendable；外面 @MainActor
        // class 持有，闭包在 audio thread 上跑——@unchecked Sendable 包一层避免 hop。
        let box = WrapperBox(wrapper: wrapper)
        AECEngine.shared.addListener(id: Self.listenerID, format: format) { buffer in
            // audio thread。VADWrapper 内部带锁，调它的 processAudioDataWithBuffer:count: 安全。
            guard let ch = buffer.floatChannelData?[0] else { return }
            let frames = UInt(buffer.frameLength)
            guard frames > 0 else { return }
            box.wrapper.processAudioData(withBuffer: ch, count: frames)
        }
        logger.notice("VAD started")
    }

    func stop() {
        guard running else { return }
        running = false
        AECEngine.shared.removeListener(id: Self.listenerID)
        logger.notice("VAD stopped")
    }
}

/// VADWrapper 装箱让 audio thread 的 Sendable 闭包可以捕获——库内部自加锁。
private final class WrapperBox: @unchecked Sendable {
    let wrapper: VADWrapper
    init(wrapper: VADWrapper) { self.wrapper = wrapper }
}

// MARK: - VADDelegate

extension VAD: VADDelegate {
    nonisolated func voiceStarted() {
        Task { @MainActor [weak self] in
            self?.onSpeechStart()
        }
    }

    nonisolated func voiceEnded(withWavData _: Data!) {
        // wavData 我们不关心——audio 已经直接喂给 Recognizer 的 PCMBuffer。
        // 只用这个回调当「说完了」信号。
        Task { @MainActor [weak self] in
            self?.onSpeechEnd()
        }
    }

    nonisolated func voiceDidContinue(withPCMFloat _: Data!) {
        // 不用——音频喂入由 AECEngine fan-out 走 Recognizer 那条路径。
    }
}
