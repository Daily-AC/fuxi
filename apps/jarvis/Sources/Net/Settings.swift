import Foundation
import AppKit

/// 用户配置：base URL（fuxi-im 地址）+ pair token + 唤醒词模式 + 热键。
///
/// pair token 走 Keychain（`SecItem*` API），其余明文 UserDefaults。Keychain 包装在
/// `Keychain.swift`——这里只持值不持密。
struct Settings: Equatable {
    /// fuxi-im 服务地址，默认本机。家用部署填 `https://im.qmledmq.cn:8443`。
    var baseURL: String
    /// pair token（一次性配对，详见 fuxi-im /api/auth/pair）。Keychain 取出后填这。
    var pairToken: String
    /// 唤醒方式：`.hotkey` / `.wakeWord` / `.both`
    var triggerMode: TriggerMode
    /// Picovoice access key（Porcupine 用）；wakeWord 时必填。
    var picovoiceKey: String
    /// 全局热键 — 用人类可读字符串保存（如 `cmd+shift+j`），运行时再 parse。
    var hotkey: HotkeyCombo
    /// TTS 语言/音色，默认普通话女声。
    var ttsVoice: String

    enum TriggerMode: String, CaseIterable, Identifiable {
        case hotkey, wakeWord, both
        var id: String { rawValue }
        var label: String {
            switch self {
            case .hotkey: return "全局热键"
            case .wakeWord: return "唤醒词「玄女」"
            case .both: return "热键 + 唤醒词"
            }
        }
    }

    static let userDefaultsKey = "cn.qmledmq.fuxi.jarvis.settings"

    static let `default` = Settings(
        baseURL: "http://127.0.0.1:9100",
        pairToken: "",
        triggerMode: .both,
        picovoiceKey: "",
        hotkey: HotkeyCombo(modifiers: [.option, .shift], keyCode: 49 /* space */),
        ttsVoice: "zh-CN"
    )

    static func load() -> Settings {
        let d = UserDefaults.standard
        guard let data = d.data(forKey: Self.userDefaultsKey),
              let dec = try? JSONDecoder().decode(SettingsCodable.self, from: data)
        else {
            return .default
        }
        return Settings(
            baseURL: dec.baseURL,
            pairToken: Keychain.load(account: "pairToken") ?? "",
            triggerMode: TriggerMode(rawValue: dec.triggerMode) ?? .both,
            picovoiceKey: Keychain.load(account: "picovoiceKey") ?? "",
            hotkey: dec.hotkey,
            ttsVoice: dec.ttsVoice
        )
    }

    func save() {
        let enc = SettingsCodable(
            baseURL: baseURL,
            triggerMode: triggerMode.rawValue,
            hotkey: hotkey,
            ttsVoice: ttsVoice
        )
        if let data = try? JSONEncoder().encode(enc) {
            UserDefaults.standard.set(data, forKey: Self.userDefaultsKey)
        }
        Keychain.save(account: "pairToken", value: pairToken)
        Keychain.save(account: "picovoiceKey", value: picovoiceKey)
    }
}

/// JSON 持久化的子集——secret 不进 UserDefaults，单独走 Keychain。
private struct SettingsCodable: Codable {
    var baseURL: String
    var triggerMode: String
    var hotkey: HotkeyCombo
    var ttsVoice: String
}

/// 热键的 modifier + keyCode 组合。Codable 直接 JSON 持久化。
struct HotkeyCombo: Codable, Equatable {
    var modifiers: [Modifier]
    var keyCode: UInt16

    enum Modifier: String, Codable, CaseIterable {
        case command, option, shift, control
        var nsFlag: NSEvent.ModifierFlags {
            switch self {
            case .command: return .command
            case .option: return .option
            case .shift: return .shift
            case .control: return .control
            }
        }
    }

    var nsFlags: NSEvent.ModifierFlags {
        modifiers.reduce(NSEvent.ModifierFlags()) { $0.union($1.nsFlag) }
    }
}
