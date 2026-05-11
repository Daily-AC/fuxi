import XCTest
@testable import Jarvis

/// fuxi-im WS 下发的 JSON 跟 WireEvent 对得上——后端 EventKind 是 serde tag enum，
/// `xuannv_voice_line` 是 tag。`agent_responded` 等其它 tag 必须降级为 `.other` 不
/// 触发任何 App 行为（PWA 才管文字流，Jarvis 只管语音侧）。
final class WireEventTests: XCTestCase {
    func test_decodeXuannvVoiceLine() throws {
        let json = """
        {
          "meta": {
            "id": "00000000-0000-0000-0000-000000000001",
            "agent": "00000000-0000-0000-0000-000000000002",
            "at": "2026-05-10T00:00:00Z"
          },
          "kind": {
            "type": "xuannv_voice_line",
            "text": "好的，已派给鲁班"
          }
        }
        """.data(using: .utf8)!
        let ev = try JSONDecoder().decode(WireEvent.self, from: json)
        guard case .voiceLine(let text) = ev.kind else {
            XCTFail("expect voiceLine, got \(ev.kind)")
            return
        }
        XCTAssertEqual(text, "好的，已派给鲁班")
    }

    func test_decodeUnknownKindFallsBackToOther() throws {
        let json = """
        {
          "meta": { "id": "00000000-0000-0000-0000-000000000001",
                    "agent": "00000000-0000-0000-0000-000000000002",
                    "at": "2026-05-10T00:00:00Z" },
          "kind": { "type": "agent_responded", "text": "其它事件" }
        }
        """.data(using: .utf8)!
        let ev = try JSONDecoder().decode(WireEvent.self, from: json)
        if case .other = ev.kind {
            // good
        } else {
            XCTFail("非 xuannv_voice_line 必须降级为 .other")
        }
    }
}
