import XCTest
@testable import Jarvis

// 协议帧编解码测试。不起 WebSocket、不连真 server——home 端 wake-server 上线后再补集成测。
//
// 这层测试守的是 WAKE_PROTOCOL.md 的 wire 形态：每加一条 type 必须先在这里覆盖编/解，
// 防止跟 home 端协议漂移（参考 fuxi 项目教训：mock fixture 必须跟真 wire 一致）。
final class RemoteWakeClientTests: XCTestCase {

    // MARK: 上行帧 encode 形态

    func test_encodeHello() throws {
        let frame = WakeFrame.hello(client: "jarvis-mac", version: "0.1.0")
        let json = try frame.encode()
        // sortedKeys 保证字段稳定顺序，便于做 string 等值比对
        XCTAssertEqual(json, #"{"client":"jarvis-mac","type":"hello","version":"0.1.0"}"#)
    }

    func test_encodeBye() throws {
        XCTAssertEqual(try WakeFrame.bye.encode(), #"{"type":"bye"}"#)
    }

    func test_encodePong() throws {
        let json = try WakeFrame.pong(at: "2026-05-10T12:00:00Z").encode()
        XCTAssertEqual(json, #"{"at":"2026-05-10T12:00:00Z","type":"pong"}"#)
    }

    func test_encodeKeywords() throws {
        let json = try WakeFrame.keywords(["玄女", "贾维斯"]).encode()
        XCTAssertEqual(json, #"{"type":"keywords","words":["玄女","贾维斯"]}"#)
    }

    // MARK: 下行帧 decode

    func test_decodeReady() {
        let f = WakeFrame.decode(from: #"{"type":"ready","keywords":["玄女"]}"#)
        guard case .ready(let kws) = f else { return XCTFail("expect ready") }
        XCTAssertEqual(kws, ["玄女"])
    }

    func test_decodeWake() {
        let f = WakeFrame.decode(from:
            #"{"type":"wake","keyword":"玄女","score":0.85,"at":"2026-05-10T12:00:01Z"}"#)
        guard case .wake(let keyword, let score, let at) = f else { return XCTFail("expect wake") }
        XCTAssertEqual(keyword, "玄女")
        XCTAssertEqual(score, 0.85, accuracy: 0.0001)
        XCTAssertEqual(at, "2026-05-10T12:00:01Z")
    }

    func test_decodeWakeWithIntScore() {
        // score 整数也应该被吃下来——server 用 1 / 0 表示二值时常见。
        let f = WakeFrame.decode(from:
            #"{"type":"wake","keyword":"贾维斯","score":1,"at":"2026-05-10T12:00:01Z"}"#)
        guard case .wake(_, let score, _) = f else { return XCTFail("expect wake") }
        XCTAssertEqual(score, 1.0, accuracy: 0.0001)
    }

    func test_decodePing() {
        let f = WakeFrame.decode(from: #"{"type":"ping","at":"2026-05-10T12:00:02Z"}"#)
        guard case .ping(let at) = f else { return XCTFail("expect ping") }
        XCTAssertEqual(at, "2026-05-10T12:00:02Z")
    }

    func test_decodeError() {
        let f = WakeFrame.decode(from:
            #"{"type":"error","code":"unauthorized","message":"token 过期"}"#)
        guard case .error(let code, let message) = f else { return XCTFail("expect error") }
        XCTAssertEqual(code, "unauthorized")
        XCTAssertEqual(message, "token 过期")
    }

    func test_decodeServerBye() {
        let f = WakeFrame.decode(from: #"{"type":"bye"}"#)
        guard case .serverBye = f else { return XCTFail("expect serverBye") }
    }

    func test_decodeUnknownTypeReturnsNil() {
        let f = WakeFrame.decode(from: #"{"type":"telemetry","load":0.5}"#)
        XCTAssertNil(f)
    }

    func test_decodeMalformedReturnsNil() {
        XCTAssertNil(WakeFrame.decode(from: "not json"))
        XCTAssertNil(WakeFrame.decode(from: "{}"))
    }

    // MARK: round-trip

    func test_roundTripPong() throws {
        let original = WakeFrame.pong(at: "2026-05-10T12:00:00Z")
        let json = try original.encode()
        let decoded = WakeFrame.decode(from: json)
        XCTAssertEqual(decoded, original)
    }

    func test_roundTripKeywords() throws {
        let original = WakeFrame.keywords(["玄女", "贾维斯", "鲁班"])
        let json = try original.encode()
        let decoded = WakeFrame.decode(from: json)
        XCTAssertEqual(decoded, original)
    }
}
