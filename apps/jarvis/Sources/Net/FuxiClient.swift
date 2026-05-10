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
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "fuxi")
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
        // 同步注入 cookie——必须在 webSocketTask 之前完成，否则 race（异步 Task 会让
        // handshake 不带 cookie），server 401 关连接，反复重连永远拿不到 voice line。
        injectCookieIfNeeded()
        // URLSessionWebSocketTask 在 macOS 14 不会自动把 cookie storage 里的 cookie 加到
        // upgrade 请求 header（普通 dataTask 会，WS task 不会，已知 darwin 行为差异）。
        // 这里手塞 Cookie header 兜底。
        var req = URLRequest(url: url)
        if let baseURL = URL(string: settings.baseURL),
           let cookies = HTTPCookieStorage.shared.cookies(for: baseURL),
           !cookies.isEmpty {
            for (k, v) in HTTPCookie.requestHeaderFields(with: cookies) {
                req.setValue(v, forHTTPHeaderField: k)
            }
        }
        let task = session.webSocketTask(with: req)
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
                self.logger.error("WS recv 失败: \(error.localizedDescription, privacy: .public)")
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
                self.logger.notice("WS conv 收一帧")
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
        guard let data else { return }
        guard let event = try? JSONDecoder().decode(WireEvent.self, from: data) else {
            // decode 失败也想知道——可能是新 EventKind 没解
            let preview = String(data: data.prefix(120), encoding: .utf8) ?? "<bin>"
            logger.notice("WS conv decode 失败 preview=\(preview, privacy: .public)")
            return
        }
        if let conv = event.toConvEvent() {
            logger.notice("WS conv 收 voice line len=\(self.text(of: conv).count, privacy: .public)")
            onEvent(conv)
        } else {
            logger.debug("WS conv 收非 voice line type=\(event.kind.kindLabel, privacy: .public)")
        }
    }

    private func text(of e: ConvEvent) -> String {
        switch e {
        case .voiceLine(let t): return t
        }
    }

    func sendIntervene(text: String) async throws {
        logger.notice("sendIntervene start len=\(text.count, privacy: .public)")
        guard let url = URL(string: settings.baseURL + "/api/intervene") else {
            throw URLError(.badURL)
        }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        // 保险起见手塞 Cookie header——URLSession 自动从 storage 注入对 dataTask 一般是稳的，
        // 但跟 WS 同样的兜底，避免 process 启动早期 cookie 还没到位的 race。
        if let baseURL = URL(string: settings.baseURL),
           let cookies = HTTPCookieStorage.shared.cookies(for: baseURL),
           !cookies.isEmpty {
            for (k, v) in HTTPCookie.requestHeaderFields(with: cookies) {
                req.setValue(v, forHTTPHeaderField: k)
            }
        }
        let body: [String: Any] = ["text": text, "mode": "append"]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (_, resp) = try await session.data(for: req)
        if let http = resp as? HTTPURLResponse {
            logger.notice("sendIntervene done HTTP \(http.statusCode, privacy: .public)")
            if !(200..<300).contains(http.statusCode) {
                throw NSError(domain: "fuxi", code: http.statusCode,
                              userInfo: [NSLocalizedDescriptionKey: "intervene HTTP \(http.statusCode)"])
            }
        }
    }

    private func injectCookieIfNeeded() {
        // 已有 cookie 就跳过——HTTPCookieStorage 自带过期管理。
        guard let url = URL(string: settings.baseURL) else { return }
        if let cookies = HTTPCookieStorage.shared.cookies(for: url),
           cookies.contains(where: { $0.name == "fuxi_im_token" }) {
            return
        }
        guard !settings.pairToken.isEmpty else { return }
        // **语义变更**：fuxi-im server 端 pair handshake 已经从「POST {"token":"…"}」改成
        // 6 位 PIN + device_name 流程（详见 fuxi-im/src/handlers/auth.rs PairBody），且
        // PWA 现在没暴露 pair UI。日常路径是用户在 mac 上自己跑：
        //
        //   curl -i -X POST https://<im>/api/auth/login \
        //     -H 'Content-Type: application/json' \
        //     -d '{"password":"…","device_name":"jarvis-mac"}'
        //
        // 把 Set-Cookie 里的 fuxi_im_token 值复制到 UserDefaults pairToken。这里直接当
        // cookie 值塞进 HTTPCookieStorage，省一次自己重发 login 持密码的安全负担。
        let value = settings.pairToken
        guard let cookie = HTTPCookie(properties: [
            .domain: url.host ?? "",
            .path: "/",
            .name: "fuxi_im_token",
            .value: value,
            .secure: url.scheme == "https",
            // server 给的 Max-Age 是 365 天；这里同步一年过期，过期前 jarvis 走 cookie，
            // 过期后用户重新 login 一次粘 token 即可。
            .expires: Date().addingTimeInterval(365 * 86400),
        ]) else {
            logger.error("注入 fuxi_im_token cookie 失败——URL host 解析异常")
            return
        }
        HTTPCookieStorage.shared.setCookie(cookie)
        logger.info("注入 fuxi_im_token cookie len=\(value.count, privacy: .public)")
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
    /// 别的 EventKind 类型——保留 type 字符串好做诊断日志，确认玄女是否真在 emit。
    case other(String)

    private enum CodingKeys: String, CodingKey { case type, text }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "xuannv_voice_line":
            let text = try c.decode(String.self, forKey: .text)
            self = .voiceLine(text)
        default:
            self = .other(type)
        }
    }

    var kindLabel: String {
        switch self {
        case .voiceLine: return "xuannv_voice_line"
        case .other(let t): return t
        }
    }
}

/// App 内消化的事件——目前只有 voiceLine。后续若要监听 task 状态可扩。
enum ConvEvent {
    case voiceLine(String)
}
