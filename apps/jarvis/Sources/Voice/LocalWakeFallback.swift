import Foundation
import Speech
@preconcurrency import AVFoundation
import OSLog

// 兜底唤醒：远端 wake-server 失联时切到本机持续监听 SFSpeechRecognizer。
//
// 设计考虑：
//   - SFSpeechRecognitionTask 单次最长 60s（Apple 限），所以 50s 主动 finalize 起新 task 接力，
//     避免 rolling 缝隙。
//   - 音频走 AECEngine.shared 的 fan-out listener——和 wake/recognizer/vad 共享同一条 vpio
//     audio engine，AEC/AGC/降噪自动吃到。listener id "fallback"。
//   - 触发后立刻 stop()——释放 listener 让主 Recognizer 起；fallback 只在 wakeMode == .fallback
//     期间活。
//   - 关键词命中规则在 `isWakeKeywordHit`，纯函数+单测覆盖；详见函数注释。
@MainActor
final class LocalWakeFallback {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "wake-fallback")
    private let recognizer: SFSpeechRecognizer?
    private let onWake: () -> Void

    /// audio thread 从 listener 闭包读 request、转发 buffer.append；main actor 在 rotate
    /// 时换成新 request。NSLock 守一个非 Sendable 引用——SFSpeech 自己保证 append 多线程安全。
    private let requestHolder = RequestHolder()
    private var task: SFSpeechRecognitionTask?
    private var rotateTimer: Timer?
    private var keywords: [String]
    private var running = false
    /// AECEngine listener id——进程里只一个 LocalWakeFallback，hardcode 即可。
    nonisolated static let listenerID = "fallback"

    // 50s 接力——SFSpeech 单 task 上限 60s（Apple 文档），留 10s 余量。
    private static let rotateInterval: TimeInterval = 50

    init(keywords: [String], onWake: @escaping () -> Void) {
        self.recognizer = SFSpeechRecognizer(locale: Locale(identifier: "zh-CN"))
        self.keywords = keywords
        self.onWake = onWake
    }

    func updateKeywords(_ keywords: [String]) {
        self.keywords = keywords
    }

    func start() {
        guard !running else { return }
        guard let recognizer, recognizer.isAvailable else {
            logger.warning("zh-CN recognizer 不可用，fallback 启动失败")
            return
        }
        do {
            try AECEngine.shared.start()
        } catch {
            logger.error("AECEngine start 失败: \(error.localizedDescription, privacy: .public)")
            return
        }
        running = true
        installListener()
        startTask()
        let timer = Timer(timeInterval: Self.rotateInterval, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.rotate() }
        }
        RunLoop.main.add(timer, forMode: .common)
        rotateTimer = timer
    }

    func stop() {
        running = false
        rotateTimer?.invalidate()
        rotateTimer = nil
        cleanupTask()
        AECEngine.shared.removeListener(id: Self.listenerID)
    }

    /// 注册 AECEngine listener：format 传 nil 拿 vpio 原生 Float32（SFSpeech 接受任意 PCM
    /// 格式，AVAudioPCMBuffer 自带 format 描述，append 时会按需消化）。
    private func installListener() {
        let holder = self.requestHolder
        AECEngine.shared.addListener(id: Self.listenerID, format: nil) { buffer in
            // audio thread——request.append 文档说 thread-safe（SFSpeech 内部排队）。
            holder.append(buffer)
        }
    }

    private func rotate() {
        guard running else { return }
        // finalize 当前 task 让结果落定，再起新的——listener 不动，新 request 接管。
        requestHolder.endAudio()
        cleanupTask()
        startTask()
    }

    private func startTask() {
        guard let recognizer else { return }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        if recognizer.supportsOnDeviceRecognition {
            request.requiresOnDeviceRecognition = true
        }
        requestHolder.set(request)

        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            guard let self else { return }
            Task { @MainActor in
                if let error {
                    // task 因为 endAudio 正常结束也会抛 error——running 还在就让 rotate 接力。
                    self.logger.debug("task end: \(error.localizedDescription, privacy: .public)")
                    return
                }
                guard let result else { return }
                let text = result.bestTranscription.formattedString
                self.checkKeywords(in: text)
            }
        }
    }

    private func checkKeywords(in transcript: String) {
        for keyword in keywords {
            if isWakeKeywordHit(transcript: transcript, keyword: keyword) {
                logger.info("fallback hit: \(keyword, privacy: .public)")
                // 立即停掉自己释放麦——主 Recognizer 马上要起。
                stop()
                onWake()
                return
            }
        }
    }

    private func cleanupTask() {
        task?.cancel()
        task = nil
        requestHolder.set(nil)
    }
}

/// audio thread 跟 main actor 共享的 SFSpeechRequest 槽位。SFSpeech 的 append/endAudio
/// 文档说 thread-safe（它自己排队），这里只需要保证 swap 原子。
private final class RequestHolder: @unchecked Sendable {
    private let lock = NSLock()
    private var request: SFSpeechAudioBufferRecognitionRequest?

    func set(_ req: SFSpeechAudioBufferRecognitionRequest?) {
        lock.lock(); defer { lock.unlock() }
        request = req
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

// 关键词命中规则——纯函数，可单测。
//
// 规则（team-lead 拍板的 B-tightened，只看左侧）：
//   命中条件：keyword 出现在 transcript 里，**且其紧邻左侧字符是
//   [string 起始 / ASCII / 标点 / 空白 / 非中文 Unicode] 之一。右侧不做约束**。
//
// 理由：呼语前一般有停顿 / 句首 / 非中文邻接；右侧跟语气词/命令是自然口语
// （「玄女啊」「玄女帮我...」）不该过滤。左侧严格只伤"间接陈述句里呼到名字"
// 这种边缘场景（「叫玄女」），赚误触发率。
//
// 多次出现取任一 hit 即可——一句话内重复出现也只触发一次（上层 stop() 释放麦）。
//
// 复杂度：O(n*m)，n=transcript 长，m=keyword 长。对 5-30 字短语足够。
public func isWakeKeywordHit(transcript: String, keyword: String) -> Bool {
    guard !keyword.isEmpty, !transcript.isEmpty else { return false }
    var searchRange = transcript.startIndex..<transcript.endIndex
    while let r = transcript.range(of: keyword, range: searchRange) {
        let leftClean = (r.lowerBound == transcript.startIndex)
            || !isChineseChar(transcript[transcript.index(before: r.lowerBound)])
        if leftClean { return true }
        searchRange = r.upperBound..<transcript.endIndex
    }
    return false
}

// 判断字符是否落在 CJK 主要 Unicode block 里。`Character.unicodeScalars` 可能多 scalar，
// 任何一个 scalar 在中文区就算中文邻接。
private func isChineseChar(_ ch: Character) -> Bool {
    for scalar in ch.unicodeScalars where isChineseScalar(scalar) {
        return true
    }
    return false
}

private func isChineseScalar(_ s: Unicode.Scalar) -> Bool {
    let v = s.value
    // CJK Unified Ideographs 主区
    if v >= 0x4E00 && v <= 0x9FFF { return true }
    // Extension A
    if v >= 0x3400 && v <= 0x4DBF { return true }
    // Extension B
    if v >= 0x20000 && v <= 0x2A6DF { return true }
    // CJK Compatibility Ideographs
    if v >= 0xF900 && v <= 0xFAFF { return true }
    return false
}
