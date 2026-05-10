import Foundation
// AVFoundation 一堆类（AVAudioPCMBuffer/AVAudioConverter）尚未 Sendable 化。@preconcurrency
// 抑制误报，逻辑上 audio tap 回调和 convert 闭包都跑在同一个 audio thread 上，没有跨线程。
@preconcurrency import AVFoundation
import OSLog

// 远端唤醒（home `fuxi-wake-server` ↔ mac）。
//
// 协议契约见 apps/jarvis/WAKE_PROTOCOL.md。这里只负责 mac 端：
//   - URLSessionWebSocketTask 连 ws[s]:///api/wake，Bearer token 走 URLRequest header
//   - 第一帧 hello JSON；server 回 ready 后才开始推 PCM
//   - AVAudioEngine 录系统输入 → AVAudioConverter 重采样到 16kHz mono PCM s16le → 40ms/帧（640 samples = 1280 bytes）
//   - server 5s 一次 ping 必须 5s 内回 pong；30s 无下行视为断线
//   - 断线退避 1/2/4/8/16/30 s；3 次失败发 .fallbackRequested 让 AppState 切兜底
//
// 选用 URLSessionWebSocketTask（不是第三方 SwiftNIO）原因：
//   1) 系统库零依赖，App ad-hoc 签名更省事
//   2) 自带 cookie/proxy 配置，跟 FuxiClient 同款绕 Clash TUN
//   3) Bearer 走 URLRequest header（webSocketTask(with: request) 形态）

/// Remote wake protocol 的帧——上下行都过这。Codable 用于单测确定性比对。
enum WakeFrame: Equatable {
    // 上行
    case hello(client: String, version: String)
    case bye
    case pong(at: String)
    case keywords([String])
    // 下行
    case ready(keywords: [String])
    case wake(keyword: String, score: Double, at: String)
    case ping(at: String)
    case error(code: String, message: String)
    case serverBye

    /// 编码为 server 期望的 JSON。返回 String 便于走 WebSocket text 帧。
    func encode() throws -> String {
        let obj: [String: Any]
        switch self {
        case .hello(let client, let version):
            obj = ["type": "hello", "client": client, "version": version]
        case .bye, .serverBye:
            obj = ["type": "bye"]
        case .pong(let at):
            obj = ["type": "pong", "at": at]
        case .keywords(let words):
            obj = ["type": "keywords", "words": words]
        case .ready(let keywords):
            obj = ["type": "ready", "keywords": keywords]
        case .wake(let keyword, let score, let at):
            obj = ["type": "wake", "keyword": keyword, "score": score, "at": at]
        case .ping(let at):
            obj = ["type": "ping", "at": at]
        case .error(let code, let message):
            obj = ["type": "error", "code": code, "message": message]
        }
        let data = try JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
        return String(data: data, encoding: .utf8) ?? ""
    }

    /// 解析 server 下行 JSON。未知 type 返 nil 让上层忽略，不抛错。
    static func decode(from text: String) -> WakeFrame? {
        guard let data = text.data(using: .utf8),
              let any = try? JSONSerialization.jsonObject(with: data),
              let dict = any as? [String: Any],
              let type = dict["type"] as? String
        else { return nil }
        switch type {
        case "ready":
            let kws = dict["keywords"] as? [String] ?? []
            return .ready(keywords: kws)
        case "wake":
            guard let keyword = dict["keyword"] as? String,
                  let at = dict["at"] as? String
            else { return nil }
            // score 可能整数也可能 double——容错
            let score = (dict["score"] as? Double)
                ?? (dict["score"] as? NSNumber).map { $0.doubleValue }
                ?? 0
            return .wake(keyword: keyword, score: score, at: at)
        case "ping":
            let at = dict["at"] as? String ?? ""
            return .ping(at: at)
        case "pong":
            return .pong(at: dict["at"] as? String ?? "")
        case "error":
            let code = dict["code"] as? String ?? "unknown"
            let message = dict["message"] as? String ?? ""
            return .error(code: code, message: message)
        case "bye":
            return .serverBye
        case "hello":
            // 上行帧也能被解（测试 round-trip 用）
            return .hello(client: dict["client"] as? String ?? "",
                          version: dict["version"] as? String ?? "")
        case "keywords":
            return .keywords(dict["words"] as? [String] ?? [])
        default:
            return nil
        }
    }
}

/// Wake client 对外抛的事件。AppState 订阅它调度听写 / 切 fallback。
enum WakeClientEvent {
    case wakeDetected(keyword: String)
    /// 重连失败超过阈值——切 LocalWakeFallback。
    case fallbackRequested
    /// 主连恢复——AppState 收到后停 fallback。
    case reconnected
}

/// 判断 WS 错误是不是「我们自己 cancel 自己」——connect() 进门会 cancelWS() 把上一条 WS
/// 的 pending receive 踢出 .failure(cancelled)；如果计入 consecutiveFailures，会迅速把
/// 刚连上的好连接因「累计 3 次失败」撕掉。
///
/// 检测策略：NSURLErrorCancelled (-999) + description 文本兜底。WebSocketTask 取消时
/// 错误 domain 不一定是 NSURLErrorDomain（实测有 NSPOSIXErrorDomain 89），所以 description
/// 兜一层。
func isSelfCancellation(_ error: Error) -> Bool {
    let nserr = error as NSError
    if nserr.domain == NSURLErrorDomain, nserr.code == NSURLErrorCancelled {
        return true
    }
    let desc = nserr.localizedDescription.lowercased()
    if desc.contains("cancel") || nserr.localizedDescription.contains("已取消") {
        return true
    }
    return false
}

@MainActor
final class RemoteWakeClient: NSObject, URLSessionWebSocketDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "wake-remote")
    private let onEvent: (WakeClientEvent) -> Void

    private var serverURL: URL
    private var bearer: String

    private var session: URLSession!
    private var ws: URLSessionWebSocketTask?
    private var ready = false

    // 重连退避：1/2/4/8/16/30 s。成功重置；3 次失败触发 fallback。
    private static let backoffLadder: [TimeInterval] = [1, 2, 4, 8, 16, 30]
    private var backoffIndex = 0
    private var consecutiveFailures = 0
    private static let fallbackThreshold = 3
    private var fallbackOpen = false

    // 任意时刻最多一个 reconnect Task；新调度前取消旧的。否则瞬时多次失败会排队多个
    // Task，每个 Task 醒来都盲调 connect()——后醒的把先醒已连上的 WS 撕掉，永远在 ready
    // 与 fallback 之间反复横跳（实测 30s 内切 5+ 次）。
    private var pendingReconnect: Task<Void, Never>?
    // 每条连接一个 generation。listen/receive 的 async 回调闭包捕获本次 generation；
    // 老 generation 的 success/failure 都直接丢——避免老 WS 的 pending recv 因为 cancelWS()
    // 触发 .failure(cancelled) 又走一遍 onWSFailure 把新连接捅死。
    private var connectionGeneration: UInt = 0

    // 心跳：server 5s 一 ping，30s 没下行帧视为断线。
    private var lastDownlinkAt = Date()
    private var heartbeatTimer: Timer?
    private static let downlinkTimeout: TimeInterval = 30

    // 音频流水线：系统硬件 format → 16kHz mono s16le。
    private let audio = AudioPipeline()

    init(serverURL: URL, bearer: String, onEvent: @escaping (WakeClientEvent) -> Void) {
        self.serverURL = serverURL
        self.bearer = bearer
        self.onEvent = onEvent
        super.init()
        let config = URLSessionConfiguration.default
        // 同 FuxiClient——Clash TUN 模式吞 127.0.0.1，proxy dict 置空让本机直连。
        config.connectionProxyDictionary = [:]
        self.session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }

    func updateEndpoint(serverURL: URL, bearer: String) {
        self.serverURL = serverURL
        self.bearer = bearer
        reconnect()
    }

    func connect() {
        logger.notice("connect → \(self.serverURL.absoluteString, privacy: .public) bearerLen=\(self.bearer.count)")
        // 进门先取消任何排队中的 reconnect——我们正在主动建连，那些 Task 都过期了。
        pendingReconnect?.cancel()
        pendingReconnect = nil
        cancelWS()
        connectionGeneration &+= 1
        let myGen = connectionGeneration
        var req = URLRequest(url: serverURL)
        req.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
        let task = session.webSocketTask(with: req)
        ws = task
        ready = false
        lastDownlinkAt = Date()
        task.resume()
        listen(generation: myGen)
        sendHello()
        startHeartbeatWatcher()
    }

    func reconnect() {
        cancelWS()
        connect()
    }

    func stop() {
        pendingReconnect?.cancel()
        pendingReconnect = nil
        cancelWS()
        audio.stop()
        heartbeatTimer?.invalidate()
        heartbeatTimer = nil
    }

    /// 暂停音频上传——保留 WS 连接（继续收 ping/wake event），但不抢麦克风让
    /// SFSpeechRecognizer 用。AppState.startListening 调一次。
    func pauseAudio() {
        logger.notice("pauseAudio (释放麦给 SFSpeechRecognizer)")
        audio.stop()
    }

    /// 恢复音频上传——AppState.stopListening / cancelToIdle 调。
    func resumeAudio() {
        guard ready else { return }
        logger.notice("resumeAudio")
        audio.start { [weak self] data in
            Task { @MainActor in self?.sendAudio(data) }
        }
    }

    private func cancelWS() {
        ws?.cancel(with: .goingAway, reason: nil)
        ws = nil
        audio.stop()
    }

    private func sendHello() {
        sendFrame(.hello(client: "jarvis-mac", version: "0.1.0"))
    }

    private func listen(generation: UInt) {
        ws?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let error):
                Task { @MainActor in
                    // 已经换 generation 了：老 WS 的 pending recv 在收尾，事件都丢——它们
                    // 自己就是被 cancelWS 踢出的，重复计数会把新连接撕掉。
                    guard self.connectionGeneration == generation else { return }
                    self.onWSFailure(error)
                }
            case .success(let msg):
                Task { @MainActor in
                    guard self.connectionGeneration == generation else { return }
                    self.lastDownlinkAt = Date()
                    self.handle(msg)
                    self.listen(generation: generation)
                }
            }
        }
    }

    private func handle(_ message: URLSessionWebSocketTask.Message) {
        let text: String?
        switch message {
        case .string(let s): text = s
        case .data(let d): text = String(data: d, encoding: .utf8)
        @unknown default: text = nil
        }
        guard let text, let frame = WakeFrame.decode(from: text) else { return }
        switch frame {
        case .ready(let keywords):
            logger.info("ready, keywords=\(keywords, privacy: .public)")
            ready = true
            // 复连成功：清失败计数 + 退避；排队中的 reconnect Task 一并作废（不然下一秒
            // 它醒来会把这条好连接重新撕掉）。如之前已开 fallback 通知 AppState 切回。
            backoffIndex = 0
            consecutiveFailures = 0
            pendingReconnect?.cancel()
            pendingReconnect = nil
            if fallbackOpen {
                fallbackOpen = false
                onEvent(.reconnected)
            }
            audio.start { [weak self] data in
                Task { @MainActor in self?.sendAudio(data) }
            }
        case .wake(let keyword, let score, _):
            logger.notice("wake! keyword=\(keyword, privacy: .public) score=\(score)")
            onEvent(.wakeDetected(keyword: keyword))
        case .ping(let at):
            // 必须 5s 内回 pong——直接同步发，下次 receive 会刷新 lastDownlinkAt。
            sendFrame(.pong(at: at.isEmpty ? RFC3339.now() : at))
        case .error(let code, let message):
            logger.error("server error: \(code, privacy: .public) \(message, privacy: .public)")
            // 协议表里 unauthorized/sdk_unavailable 立即切 fallback；rate_limited 走退避；
            // audio_format_invalid 不重连（开发期 bug）。
            switch code {
            case "audio_format_invalid":
                cancelWS()
            case "unauthorized", "sdk_unavailable":
                tripFallback()
            default:
                onWSFailure(NSError(domain: "wake", code: 0,
                                    userInfo: [NSLocalizedDescriptionKey: code]))
            }
        case .serverBye:
            // 服务端主动断（升级/重启）——立即按一次失败走退避，不直接 fallback。
            onWSFailure(NSError(domain: "wake", code: 0,
                                userInfo: [NSLocalizedDescriptionKey: "server bye"]))
        case .hello, .pong, .keywords, .bye:
            // 上行帧不该从 server 来——忽略
            break
        }
    }

    private func sendFrame(_ frame: WakeFrame) {
        guard let ws else { return }
        do {
            let text = try frame.encode()
            ws.send(.string(text)) { [weak self] err in
                if let err {
                    self?.logger.error("send frame 失败: \(err.localizedDescription, privacy: .public)")
                }
            }
        } catch {
            logger.error("encode frame 失败: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func sendAudio(_ data: Data) {
        guard ready, let ws else { return }
        ws.send(.data(data)) { [weak self] err in
            if let err {
                self?.logger.error("send audio 失败: \(err.localizedDescription, privacy: .public)")
            }
        }
    }

    private func startHeartbeatWatcher() {
        heartbeatTimer?.invalidate()
        // 1s tick 检查 30s 无下行——比 schedulePolicy 更直观。
        let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            guard let self else { return }
            Task { @MainActor in
                if Date().timeIntervalSince(self.lastDownlinkAt) > Self.downlinkTimeout {
                    self.logger.warning("downlink idle > \(Self.downlinkTimeout)s, reconnect")
                    self.onWSFailure(NSError(domain: "wake", code: 0,
                                             userInfo: [NSLocalizedDescriptionKey: "downlink timeout"]))
                }
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        heartbeatTimer = timer
    }

    private func onWSFailure(_ error: Error) {
        // 自己 cancel 自己（cancelWS 把上一条 WS 踢出 .failure(cancelled)）不算真失败，
        // 否则一次真断会被三次自取消放大成「3 次失败 → trip fallback」，把好连接撕掉。
        if isSelfCancellation(error) {
            logger.debug("WS self-cancel ignored: \(error.localizedDescription, privacy: .public)")
            return
        }
        logger.warning("WS 失败: \(error.localizedDescription, privacy: .public)")
        cancelWS()
        consecutiveFailures += 1
        if consecutiveFailures >= Self.fallbackThreshold, !fallbackOpen {
            tripFallback()
        }
        let delay = Self.backoffLadder[min(backoffIndex, Self.backoffLadder.count - 1)]
        backoffIndex = min(backoffIndex + 1, Self.backoffLadder.count - 1)
        // 任意时刻只允许一个 reconnect 在排队——上一次的没醒就让它过期，避免堆积。
        pendingReconnect?.cancel()
        pendingReconnect = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            if Task.isCancelled { return }
            await MainActor.run { self?.connect() }
        }
    }

    private func tripFallback() {
        guard !fallbackOpen else { return }
        fallbackOpen = true
        onEvent(.fallbackRequested)
    }
}

// MARK: - 音频管线
//
// 把 vpio 输入（mono Float32，48 kHz 一般）切成 16 kHz mono PCM s16le 帧。
// 不再自起 AVAudioEngine——挂 AECEngine.shared 的 fan-out tap，AEC/AGC/降噪都吃到。
// 每攒 640 samples（40 ms @ 16 kHz）= 1280 bytes 输出一帧。
//
// listener 注册的 target format 直接是 16k s16 mono：AECEngine 在 audio thread 上做
// AVAudioConverter，pipeline 这里只需要把已经转好的 buffer 切帧上送。

@MainActor
private final class AudioPipeline {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "wake-audio")
    /// audio thread 上读写——用 NSLock 包的 buffer，listener 闭包跨 thread 安全。
    private let buffer = PendingBuffer()
    /// 40ms @ 16kHz = 640 samples * 2 bytes = 1280 bytes（wake 协议帧大小）。
    /// nonisolated：audio thread 闭包要读，不能 main-actor hop。
    nonisolated static let frameBytes = 1280
    /// AECEngine listener id——进程内只一个 RemoteWakeClient 实例，hardcode 即可。
    nonisolated static let listenerID = "wake"

    func start(onFrame: @escaping @Sendable (Data) -> Void) {
        stop()
        guard let outFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16000,
            channels: 1,
            interleaved: true
        ) else {
            logger.error("无法构造目标音频格式")
            return
        }
        do {
            try AECEngine.shared.start()
        } catch {
            logger.error("AECEngine start 失败: \(error.localizedDescription, privacy: .public)")
            return
        }
        let buffer = self.buffer
        AECEngine.shared.addListener(id: Self.listenerID, format: outFormat) { pcm in
            // audio thread 调。pcm 已 16k s16 mono，切 1280 字节一帧上送。
            guard let int16Channel = pcm.int16ChannelData?[0] else { return }
            let count = Int(pcm.frameLength) * 2
            if count == 0 { return }
            let chunk = Data(bytes: int16Channel, count: count)
            buffer.append(chunk)
            while let frame = buffer.popFrame(of: Self.frameBytes) {
                onFrame(frame)
            }
        }
    }

    func stop() {
        AECEngine.shared.removeListener(id: Self.listenerID)
        buffer.reset()
    }
}

/// audio thread + main 都会摸——NSLock 守 Data。pop/append 简单互斥。
private final class PendingBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var data = Data()

    func append(_ chunk: Data) {
        lock.lock(); defer { lock.unlock() }
        data.append(chunk)
    }

    func popFrame(of size: Int) -> Data? {
        lock.lock(); defer { lock.unlock() }
        guard data.count >= size else { return nil }
        let frame = data.prefix(size)
        data.removeFirst(size)
        return Data(frame)
    }

    func reset() {
        lock.lock(); defer { lock.unlock() }
        data.removeAll(keepingCapacity: false)
    }
}

// 简易 RFC3339 现在时间——pong 的 at 字段用。系统 ISO8601DateFormatter 默认就是 RFC3339。
// 不缓存 static formatter，避开 Sendable 静态变量限制；pong 只在 5s 一次的心跳路径，per-call
// 构造可忽略。
enum RFC3339 {
    static func now() -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f.string(from: Date())
    }
}
