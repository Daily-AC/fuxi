import XCTest
@testable import Jarvis

final class PetSettingsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        UserDefaults.standard.removeObject(forKey: Settings.userDefaultsKey)
    }

    func test_default_uiMode_is_capsule() {
        XCTAssertEqual(Settings.default.uiMode, .capsule)
    }

    func test_uiMode_round_trip_pet() {
        var s = Settings.default
        s.uiMode = .pet
        s.save()
        let loaded = Settings.load()
        XCTAssertEqual(loaded.uiMode, .pet)
    }

    func test_legacy_uiMode_missing_falls_back_to_capsule() {
        // 老用户升级：UserDefaults 里 SettingsCodable 没有 uiMode 字段
        struct LegacyCodable: Codable {
            var baseURL: String
            var triggerMode: String
            var hotkey: HotkeyCombo
            var ttsVoice: String
        }
        let legacy = LegacyCodable(
            baseURL: "https://im.qmledmq.cn:8443",
            triggerMode: "both",
            hotkey: HotkeyCombo(modifiers: [.control, .option], keyCode: 0x2E),
            ttsVoice: ""
        )
        let data = try! JSONEncoder().encode(legacy)
        UserDefaults.standard.set(data, forKey: Settings.userDefaultsKey)
        let loaded = Settings.load()
        XCTAssertEqual(loaded.uiMode, .capsule)
    }
}
