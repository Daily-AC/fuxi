import Foundation
import AppKit

/// 用户配置：base URL（fuxi-im 地址）+ pair token + 唤醒词模式 + 热键。
///
/// secret 字段（pairToken / wakeToken / picovoiceKey）走 Keychain，其余明文 UserDefaults。
/// Keychain 包装在 `Keychain.swift`——这里只持值不持密。
struct Settings: Equatable {
    /// fuxi-im 服务地址，默认本机。家用部署填 `https://im.qmledmq.cn:8443`。
    var baseURL: String
    /// pair token（一次性配对，详见 fuxi-im /api/auth/pair）。
    var pairToken: String
    /// 唤醒方式：`.hotkey` / `.wakeWord` / `.both`
    var triggerMode: TriggerMode
    /// 远端 wake-server WebSocket 地址。本机 dev `ws://127.0.0.1:9101/api/wake`，
    /// 公网 `wss://wake.qmledmq.cn/api/wake`。
    var wakeServerURL: String
    /// 远端 wake-server Bearer token——跟 fuxi-im pair 走同一颗 secret，但 mac 端独立填。
    var wakeToken: String
    /// 兜底唤醒（LocalWakeFallback）关键词。匹配规则在 `LocalWakeFallback.isWakeKeywordHit`。
    var wakeKeywords: [String]
    /// Picovoice access key——v0.1 已不用（保留是为不丢老用户已填值），v0.2 起 UI 不暴露。
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
        wakeServerURL: "ws://127.0.0.1:9101/api/wake",
        wakeToken: "",
        wakeKeywords: ["玄女", "贾维斯"],
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
            wakeServerURL: dec.wakeServerURL ?? Self.default.wakeServerURL,
            wakeToken: Keychain.load(account: "wakeToken") ?? "",
            wakeKeywords: dec.wakeKeywords ?? Self.default.wakeKeywords,
            picovoiceKey: Keychain.load(account: "picovoiceKey") ?? "",
            hotkey: dec.hotkey,
            ttsVoice: dec.ttsVoice
        )
    }

    func save() {
        let enc = SettingsCodable(
            baseURL: baseURL,
            triggerMode: triggerMode.rawValue,
            wakeServerURL: wakeServerURL,
            wakeKeywords: wakeKeywords,
            hotkey: hotkey,
            ttsVoice: ttsVoice
        )
        if let data = try? JSONEncoder().encode(enc) {
            UserDefaults.standard.set(data, forKey: Self.userDefaultsKey)
        }
        Keychain.save(account: "pairToken", value: pairToken)
        Keychain.save(account: "wakeToken", value: wakeToken)
        Keychain.save(account: "picovoiceKey", value: picovoiceKey)
    }
}

// 字段加 Optional 留兼容——v0.1 老 UserDefaults 没有 wakeServerURL/wakeKeywords 时不会 deserialize 失败。
private struct SettingsCodable: Codable {
    var baseURL: String
    var triggerMode: String
    var wakeServerURL: String?
    var wakeKeywords: [String]?
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
