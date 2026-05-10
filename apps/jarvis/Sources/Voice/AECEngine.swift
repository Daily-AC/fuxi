import Foundation
// AVFoundation 的 PCM buffer / converter / format 都没 Sendable 化。
// 我们自己保证：tap 回调和 fan-out 闭包都跑在同一条 audio render thread 上，
// 不跨线程；@preconcurrency 抑制误报。
@preconcurrency import AVFoundation
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

@MainActor
final class AECEngine {
    static let shared = AECEngine()

    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "aec")
    private let engine = AVAudioEngine()

    /// 唯一的 input tap fan-out 调度器——audio thread 上调用其 `dispatch(_:)`。
    private let dispatcher = TapDispatcher()

    /// earcon 播放节点：attach 到 outputNode，AVAudioFile schedule 一发一响。
    private var earconPlayer: AVAudioPlayerNode?
    /// earcon 音频缓冲——一次 load，多次复用，避免每次 schedule 时的磁盘读延迟。
    private var earconBuffer: AVAudioPCMBuffer?
    private var earconFormat: AVAudioFormat?

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
        // bufferSize 1024 同原实现——vpio 实际可能给到 ~480 帧（10ms@48k），系统会按需切。
        input.installTap(onBus: 0, bufferSize: 1024, format: nativeFormat) { buffer, _ in
            // 跑在 audio render thread。dispatcher 内部已加锁，不阻塞 main。
            dispatcher.dispatch(buffer: buffer)
        }

        // earcon 播放器：attach + connect 到 mainMixer（mainMixer → outputNode 系统会自动连）。
        // 等真正 play earcon 时再 schedule buffer。
        prepareEarcon()

        engine.prepare()
        try engine.start()
        started = true
        logger.notice("AECEngine started")
    }

    func stop() {
        guard started else { return }
        engine.inputNode.removeTap(onBus: 0)
        if let player = earconPlayer {
            player.stop()
            engine.detach(player)
            earconPlayer = nil
        }
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
            if converter == nil {
                logger.error("listener=\(id, privacy: .public) AVAudioConverter 构造失败 (inFmt sr=\(inFormat.sampleRate) → outFmt sr=\(format.sampleRate))")
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

    /// 200ms「叮」短音确认唤醒——替代 TTS「我在」。走 vpio output，AEC 自动从 mic 减掉，
    /// 不会自激。落到 main mixer → outputNode → vpio 输出端。
    func playEarcon() {
        guard started, let player = earconPlayer, let buffer = earconBuffer else {
            logger.warning("playEarcon: engine 未启动或 earcon 未加载")
            return
        }
        // schedule 完立即 play。重复触发不排队——interrupt + replay。
        if player.isPlaying {
            player.stop()
        }
        player.scheduleBuffer(buffer, at: nil, options: [.interrupts])
        player.play()
    }

    // MARK: - 内部

    /// 加载 earcon：先尝试 `Resources/earcon-zen.caf`（user 后续可换）；
    /// 没找到就 fallback `/System/Library/Sounds/Tink.aiff`（macOS 系统音，必存在）。
    private func prepareEarcon() {
        let candidatePaths: [String] = [
            // app bundle 内的资源（xcodegen + Resources 目录）
            Bundle.main.url(forResource: "earcon-zen", withExtension: "caf")?.path,
            // SwiftPM `swift run` 场景：cwd 下找
            FileManager.default.currentDirectoryPath + "/Resources/earcon-zen.caf",
            // 系统兜底——macOS 14+ 这个路径是稳定 ABI
            "/System/Library/Sounds/Tink.aiff",
        ].compactMap { $0 }

        guard let path = candidatePaths.first(where: { FileManager.default.fileExists(atPath: $0) }) else {
            logger.error("找不到任何 earcon 候选音频文件")
            return
        }

        do {
            let url = URL(fileURLWithPath: path)
            let file = try AVAudioFile(forReading: url)
            let format = file.processingFormat
            let frameCount = AVAudioFrameCount(file.length)
            guard frameCount > 0,
                  let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount)
            else {
                logger.error("earcon buffer 构造失败")
                return
            }
            try file.read(into: buffer)
            earconBuffer = buffer
            earconFormat = format

            // attach + connect player → mainMixer。format 传 file 的 processingFormat
            // 而不是 outputNode format——AVAudioEngine 内部会做必要的格式转换。
            let player = AVAudioPlayerNode()
            engine.attach(player)
            engine.connect(player, to: engine.mainMixerNode, format: format)
            earconPlayer = player
            logger.notice("earcon ready: \(path, privacy: .public)")
        } catch {
            logger.error("earcon 加载失败: \(error.localizedDescription, privacy: .public)")
        }
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
