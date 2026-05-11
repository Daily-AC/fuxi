import Foundation
// AVFoundation 的 PCM buffer / converter / format 都没 Sendable 化。
// 我们自己保证：tap 回调和 fan-out 闭包都跑在同一条 audio render thread 上，
// 不跨线程；@preconcurrency 抑制误报。
@preconcurrency import AVFoundation
import AppKit
import OSLog

// MARK: - AECEngine
//
// 全局共享的 AVAudioEngine + Apple vpio audio unit（Voice Processing IO）。
// vpio 本质是 Audio Unit Subtype kAudioUnitSubType_VoiceProcessingIO——同 Siri /
// FaceTime 走的那条链：
//   - 内置 AEC（acoustic echo cancellation，扬声器播啥从 mic 减掉）
//   - 内置 AGC（automatic gain control，远近声压拉齐）
//   - 内置噪声抑制
// AVAudioInputNode.setVoiceProcessingEnabled(true) 是高层 API：engine 在 stop 状态下
// 设这个属性，会把 input + output 两端的 audio unit 一起切到 vpio。比手撸 AudioUnit
// 简洁；劣势是控制粒度低（细到 BypassVoiceProcessing 之类的 sub-property 设不了），
// 但我们这里只要"AEC + AGC + NS 全开"，高层 API 完全够。
//
// vpio 的 native 采样率走系统默认（macOS 一般 48 kHz mono Float32）。**不要**强行
// 设 16 kHz——原 wake/SFSpeech engine 是各自起 engine 自己定 inFormat，新架构里所有
// 消费者都共享 vpio engine，只能拿到 vpio 给的格式；想要 16 kHz s16 自己转。
//
// 多消费者：vpio engine 的 inputNode 只能 installTap **一次**（Apple API 硬约束）。
// 这里在内部 install 唯一一条 tap，把 buffer 广播给注册过的 listener；每个 listener
// 可选指定目标 format，AECEngine 用 per-listener 的 AVAudioConverter 转换后再送出。
//
// 鸣谢 https://github.com/kasimok/AECAudioStream（vpio 用法参考；我们走 AVAudioEngine
// 而不是它的 AudioUnit 直调）。

/// 诊断用 RMS 累积——audio thread 间共享，NSLock 保护。
private final class RMSDiagState: @unchecked Sendable {
    var count = 0
    var rmsSumSquared: Float = 0
    var rmsFrames = 0
}

@MainActor
final class AECEngine {
    static let shared = AECEngine()

    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "aec")
    private let engine = AVAudioEngine()

    /// 唯一的 input tap fan-out 调度器——audio thread 上调用其 `dispatch(_:)`。
    private let dispatcher = TapDispatcher()

    /// earcon 走独立 NSSound 而**不**接进 vpio engine：
    /// 早期实现把 AVAudioPlayerNode attach 到 engine.mainMixerNode 上，但 file 的
    /// processingFormat（Tink.aiff 是 stereo 22050）跟 vpio mainMixer 期望的 format
    /// （mono 48k Float32）撞不上，engine.start() 直接抛 -10875 把整个 mic 链路废掉。
    /// 代价：earcon 不走 AEC reference 路径，回声会被 mic 收到——但 200ms 短促音 +
    /// 用户开口窗口本来就有 SFSpeech leading timeout 6s 缓冲，影响可忽略。
    private var earconSound: NSSound?

    private var started = false

    private init() {}

    // MARK: - 生命周期

    /// 起 engine。幂等：第二次调直接 return。
    func start() throws {
        if started { return }
        let input = engine.inputNode

        // 关键：先把 vpio 切上，**必须 engine stop**。Apple 文档：「Set this before
        // starting the engine」。setVoiceProcessingEnabled 同时切 input + output 两端，
        // 不能只设单边。
        do {
            try input.setVoiceProcessingEnabled(true)
        } catch {
            // 失败不致命——掉级到普通 input/output（仍能录音/播放，只是没 AEC）。
            logger.error("vpio 启用失败，掉级普通 input: \(error.localizedDescription, privacy: .public)")
        }

        // 单 tap fan-out。format 传 nil = 用 inputNode 当前 vpio 输出 format（48k 单声 Float32）。
        let nativeFormat = input.outputFormat(forBus: 0)
        logger.notice("vpio native format: sr=\(nativeFormat.sampleRate) ch=\(nativeFormat.channelCount) common=\(nativeFormat.commonFormat.rawValue)")

        let dispatcher = self.dispatcher
        // 诊断用：每 100 个 buffer 打一次 RMS（约 1s 一次 @48k）——验证 mic 链路真有信号。
        let rmsCounter = NSLock()
        let rmsState = RMSDiagState()
        let diagLogger = self.logger
        // bufferSize 1024 同原实现——vpio 实际可能给到 ~480 帧（10ms@48k），系统会按需切。
        input.installTap(onBus: 0, bufferSize: 1024, format: nativeFormat) { buffer, _ in
            // 跑在 audio render thread。dispatcher 内部已加锁，不阻塞 main。
            dispatcher.dispatch(buffer: buffer)

            // 算第一通道 RMS 累积——audio thread 操作，counter 自旋
            rmsCounter.lock()
            rmsState.count += 1
            if let chan = buffer.floatChannelData?[0] {
                let n = Int(buffer.frameLength)
                var sum: Float = 0
                for i in 0..<n {
                    sum += chan[i] * chan[i]
                }
                rmsState.rmsSumSquared += sum
                rmsState.rmsFrames += n
            }
            let shouldLog = rmsState.count >= 100
            var rmsToReport: Float = 0
            var framesToReport = 0
            if shouldLog {
                rmsToReport = rmsState.rmsFrames > 0 ? sqrt(rmsState.rmsSumSquared / Float(rmsState.rmsFrames)) : 0
                framesToReport = rmsState.rmsFrames
                rmsState.count = 0
                rmsState.rmsSumSquared = 0
                rmsState.rmsFrames = 0
            }
            rmsCounter.unlock()
            if shouldLog {
                diagLogger.notice("mic RMS=\(rmsToReport, privacy: .public) frames=\(framesToReport, privacy: .public)")
            }
        }

        // earcon 加载——走独立 NSSound 不接 engine graph（详见 earconSound 注释）。
        prepareEarcon()

        engine.prepare()
        try engine.start()
        started = true
        logger.notice("AECEngine started")
    }

    func stop() {
        guard started else { return }
        engine.inputNode.removeTap(onBus: 0)
        earconSound?.stop()
        earconSound = nil
        engine.stop()
        started = false
        dispatcher.removeAll()
        logger.notice("AECEngine stopped")
    }

    /// vpio 的 input 原生格式——listener 不指定 format 时拿到的就是这。
    var inputFormat: AVAudioFormat {
        engine.inputNode.outputFormat(forBus: 0)
    }

    // MARK: - listener 注册

    /// 多消费者注册 input tap。
    /// - Parameters:
    ///   - id: 用来后续 remove 的 key（"wake" / "fallback" / "main-stt" / "vad"）。重复 add 同 id 会替换。
    ///   - format: 目标 PCM 格式；nil = vpio 原生（不转换，直接送原 buffer）。
    ///   - tap: audio thread 上调用，**禁止跨线程 lock 主 actor**——cross-thread state 自己处理。
    func addListener(id: String,
                     format: AVAudioFormat?,
                     _ tap: @Sendable @escaping (AVAudioPCMBuffer) -> Void) {
        let inFormat = inputFormat
        let converter: AVAudioConverter?
        if let format, format != inFormat {
            converter = AVAudioConverter(from: inFormat, to: format)
            if let conv = converter {
                // 多通道源 → 单声道目标（wake/STT/VAD 都是 mono）：默认 N→1 是平均所有通道，
                // 但用户系统装了向日葵 OrayVirtualAudioDevice 之类虚拟驱动后 vpio 可能拿到
                // 9 ch 聚合视图——其余 8 通道空，平均后信号被稀释 9 倍，wake/STT 听到的是
                // 接近静音的 mono。强制 channelMap[0] = 只取 channel 0（默认主 mic 位置），
                // 振幅不被稀释。
                if format.channelCount == 1, inFormat.channelCount > 1 {
                    conv.channelMap = [0]
                }
            } else {
                logger.error("listener=\(id, privacy: .public) AVAudioConverter 构造失败 (inFmt sr=\(inFormat.sampleRate) ch=\(inFormat.channelCount) → outFmt sr=\(format.sampleRate) ch=\(format.channelCount))")
            }
        } else {
            converter = nil
        }
        dispatcher.add(id: id, format: format, converter: converter, tap: tap)
    }

    func removeListener(id: String) {
        dispatcher.remove(id: id)
    }

    // MARK: - earcon

    /// 200ms「叮」短音确认唤醒——替代 TTS「我在」。
    ///
    /// 走 NSSound 独立路径不进 vpio engine——绕开 AVAudioFile.processingFormat 跟 vpio
    /// mainMixer expected format mismatch 的坑（实测会让 engine.start() 抛 -10875）。
    /// 代价：自激不被 AEC 减掉，但 200ms 短音影响可忽略。
    func playEarcon() {
        guard let sound = earconSound else {
            logger.warning("playEarcon: earcon 未加载")
            return
        }
        // 重复触发：先 stop 再 play 避免叠加。
        if sound.isPlaying {
            sound.stop()
        }
        sound.play()
    }

    // MARK: - 内部

    /// 加载 earcon：先尝试 `Resources/earcon-zen.caf`（user 后续可换）；
    /// 没找到就 fallback `/System/Library/Sounds/Tink.aiff`（macOS 系统音，必存在）。
    private func prepareEarcon() {
        let candidatePaths: [String] = [
            Bundle.main.url(forResource: "earcon-zen", withExtension: "caf")?.path,
            FileManager.default.currentDirectoryPath + "/Resources/earcon-zen.caf",
            "/System/Library/Sounds/Tink.aiff",
        ].compactMap { $0 }

        guard let path = candidatePaths.first(where: { FileManager.default.fileExists(atPath: $0) }),
              let sound = NSSound(contentsOfFile: path, byReference: false)
        else {
            logger.error("找不到/加载失败 earcon 候选音频文件")
            return
        }
        earconSound = sound
        logger.notice("earcon ready: \(path, privacy: .public)")
    }
}

// MARK: - TapDispatcher
//
// audio render thread 上 fan-out 单 tap buffer 到多 listener。
// 加锁理由：listener add/remove 可能在 main actor，dispatch 在 audio thread。
// NSLock 比 actor 简单——dispatch 路径不能 await。
// @unchecked Sendable：手动用 NSLock 守 listener 表，audio buffer 只读不 mutate
// （converter.convert 内部会读 inputBuffer，不修改）。
final class TapDispatcher: @unchecked Sendable {
    private let lock = NSLock()
    private var listeners: [Listener] = []

    private struct Listener {
        let id: String
        let targetFormat: AVAudioFormat?     // nil = 透传原 buffer
        let converter: AVAudioConverter?     // nil 或 targetFormat == nil 时透传
        let tap: @Sendable (AVAudioPCMBuffer) -> Void
    }

    func add(id: String,
             format: AVAudioFormat?,
             converter: AVAudioConverter?,
             tap: @Sendable @escaping (AVAudioPCMBuffer) -> Void) {
        lock.lock(); defer { lock.unlock() }
        listeners.removeAll { $0.id == id }
        listeners.append(Listener(id: id, targetFormat: format, converter: converter, tap: tap))
    }

    func remove(id: String) {
        lock.lock(); defer { lock.unlock() }
        listeners.removeAll { $0.id == id }
    }

    func removeAll() {
        lock.lock(); defer { lock.unlock() }
        listeners.removeAll()
    }

    /// 当前 listener 数量——单测可见性用。
    var count: Int {
        lock.lock(); defer { lock.unlock() }
        return listeners.count
    }

    func contains(id: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return listeners.contains { $0.id == id }
    }

    /// audio thread 调。snapshot listener 表（避开持锁回调死锁），逐 listener 派发。
    func dispatch(buffer: AVAudioPCMBuffer) {
        lock.lock()
        let snapshot = listeners
        lock.unlock()
        for l in snapshot {
            if let converter = l.converter, let outFormat = l.targetFormat {
                guard let converted = convertBuffer(buffer, using: converter, outFormat: outFormat) else {
                    continue
                }
                l.tap(converted)
            } else {
                l.tap(buffer)
            }
        }
    }

    /// 单次 buffer convert。converter 内部会按需吃多帧 + 攒输出，但我们这里每次 callback
    /// 都灌一份原始 buffer，所以用 ConvertFlag 控制只让 converter 拿到一次输入。
    private func convertBuffer(_ input: AVAudioPCMBuffer,
                               using converter: AVAudioConverter,
                               outFormat: AVAudioFormat) -> AVAudioPCMBuffer? {
        let ratio = outFormat.sampleRate / input.format.sampleRate
        let outCapacity = AVAudioFrameCount(Double(input.frameLength) * ratio + 16)
        guard outCapacity > 0,
              let outBuf = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: outCapacity)
        else {
            return nil
        }
        let flag = ConvertFlag()
        var error: NSError?
        let status = converter.convert(to: outBuf, error: &error) { [flag] _, statusOut in
            if flag.consumed {
                statusOut.pointee = .noDataNow
                return nil
            }
            flag.consumed = true
            statusOut.pointee = .haveData
            return input
        }
        if status == .error || error != nil { return nil }
        if outBuf.frameLength == 0 { return nil }
        return outBuf
    }
}

// 给 AVAudioConverter 闭包的 mutable 容器——避免 var 捕获报 Sendable 警告。
// 闭包同步执行（converter 在调用 thread 同步调），class 引用本身被传是 OK 的。
private final class ConvertFlag: @unchecked Sendable {
    var consumed = false
}
