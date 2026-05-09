import Foundation
import OSLog

/// fuxi-im 客户端：HTTP `/api/intervene` + WebSocket `/api/conv`。
///
/// 鉴权：fuxi-im 用 cookie 鉴权（COOKIE_NAME=fuxi_im_token）。配对流程：
///   1. 用户在 PWA 登录，进设置面板获取 pair token
///   2. 在贾维斯设置面板填 base URL + pair token
///   3. 客户端 POST /api/auth/pair 换 cookie
///   4. cookie 自动塞 URLSession.shared.cookieStore
///
/// **绕 Clash TUN**：URLSessionConfiguration.connectionProxyDictionary 设为空，
/// 让 127.0.0.1 不走系统代理（按 CLAUDE.md 陷阱"cc 反连 --sdk-url 被 Clash TUN 吞"
/// 同款防御）。
final class FuxiClient: NSObject, URLSessionWebSocketDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "fuxi")
    private var settings: Settings
    private let onEvent: (ConvEvent) -> Void
    private let onStatus: (String) -> Void
    private var session: URLSession!
    private var ws: URLSessionWebSocketTask?
    private var reconnectAfter: TimeInterval = 1.0

    init(settings: Settings,
         eventHandler: @escaping (ConvEvent) -> Void,
         statusHandler: @escaping (String) -> Void) {
        self.settings = settings
        self.onEvent = eventHandler
        self.onStatus = statusHandler
        super.init()
        let config = URLSessionConfiguration.default
        config.httpCookieStorage = HTTPCookieStorage.shared
        config.httpCookieAcceptPolicy = .always
        config.httpShouldSetCookies = true
        // 把 127.0.0.1 / localhost 排除出系统代理——Clash TUN 模式下 SYN 也被代理拦。
        config.connectionProxyDictionary = [:]
        self.session = URLSession(configuration: config, delegate: self, delegateQueue: nil)
    }

    func updateSettings(_ s: Settings) {
        self.settings = s
        reconnect()
    }

    func connect() {
        guard let url = wsURL() else {
            onStatus("invalid base URL")
            return
        }
        Task { await pairIfNeeded() }
        let task = session.webSocketTask(with: url)
        ws = task
        task.resume()
        onStatus("connecting…")
        listen()
    }

    func reconnect() {
        ws?.cancel(with: .goingAway, reason: nil)
        ws = nil
        connect()
    }

    private func listen() {
        ws?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let error):
                self.logger.error("WS recv 失败: \(error.localizedDescription)")
                self.onStatus("disconnected")
                // 指数退避重连——上限 30s。
                let delay = self.reconnectAfter
                self.reconnectAfter = min(self.reconnectAfter * 2, 30)
                DispatchQueue.global().asyncAfter(deadline: .now() + delay) { [weak self] in
                    self?.connect()
                }
            case .success(let msg):
                self.reconnectAfter = 1.0
                self.onStatus("connected")
                self.handle(msg)
                self.listen()
            }
        }
    }

    private func handle(_ message: URLSessionWebSocketTask.Message) {
        let data: Data?
        switch message {
        case .data(let d): data = d
        case .string(let s): data = s.data(using: .utf8)
        @unknown default: data = nil
        }
        guard let data,
              let event = try? JSONDecoder().decode(WireEvent.self, from: data)
        else { return }
        if let conv = event.toConvEvent() {
            onEvent(conv)
        }
    }

    func sendIntervene(text: String) async throws {
        guard let url = URL(string: settings.baseURL + "/api/intervene") else {
            throw URLError(.badURL)
        }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: Any] = ["text": text, "mode": "append"]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (_, resp) = try await session.data(for: req)
        if let http = resp as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw NSError(domain: "fuxi", code: http.statusCode,
                          userInfo: [NSLocalizedDescriptionKey: "intervene HTTP \(http.statusCode)"])
        }
    }

    private func pairIfNeeded() async {
        // 已有 cookie 就跳过——HTTPCookieStorage 自带过期管理。
        if let url = URL(string: settings.baseURL),
           let cookies = HTTPCookieStorage.shared.cookies(for: url),
           cookies.contains(where: { $0.name == "fuxi_im_token" }) {
            return
        }
        guard !settings.pairToken.isEmpty,
              let url = URL(string: settings.baseURL + "/api/auth/pair")
        else { return }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try? JSONSerialization.data(withJSONObject: ["token": settings.pairToken])
        do {
            let (_, resp) = try await session.data(for: req)
            if let http = resp as? HTTPURLResponse {
                logger.info("pair HTTP \(http.statusCode, privacy: .public)")
            }
        } catch {
            logger.error("pair 失败: \(error.localizedDescription)")
        }
    }

    private func wsURL() -> URL? {
        guard var components = URLComponents(string: settings.baseURL) else { return nil }
        switch components.scheme {
        case "https": components.scheme = "wss"
        case "http": components.scheme = "ws"
        default: return nil
        }
        components.path = "/api/conv"
        return components.url
    }
}

/// fuxi 后端 wire 事件——只解我们关心的字段，其它忽略。`kind.type` 是 EventKind serde tag。
struct WireEvent: Decodable {
    struct Meta: Decodable {
        let id: String?
        let agent: String?
    }
    let meta: Meta
    let kind: WireKind

    func toConvEvent() -> ConvEvent? {
        switch kind {
        case .voiceLine(let text):
            return .voiceLine(text)
        case .other:
            return nil
        }
    }
}

enum WireKind: Decodable {
    case voiceLine(String)
    case other

    private enum CodingKeys: String, CodingKey { case type, text }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "xuannv_voice_line":
            let text = try c.decode(String.self, forKey: .text)
            self = .voiceLine(text)
        default:
            self = .other
        }
    }
}

/// App 内消化的事件——目前只有 voiceLine。后续若要监听 task 状态可扩。
enum ConvEvent {
    case voiceLine(String)
}
