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
    /// TTS voice identifier。空串 = 让 Synthesizer 自动选最高质量 zh-CN voice
    /// （`AVSpeechSynthesisVoice.speechVoices()` 里 quality.rawValue 最大的）。
    /// 仅 `ttsProvider == .system` 时生效。
    var ttsVoice: String
    /// TTS 语速。`AVSpeechUtteranceDefaultSpeechRate` = 0.5；默认 0.55 提 10%。
    /// 设置面板 slider 范围 [0.4, 0.7]——再往两端会失真。
    /// 仅 `ttsProvider == .system` 时生效（远端 TTS 语速由 server 模型自身决定）。
    var ttsRate: Double
    /// TTS provider：系统语音（AVSpeechSynthesizer）/ 远端角色音色（GPT-SoVITS）。
    var ttsProvider: TTSProvider
    /// 远端 TTS API URL。默认走 fuxi-im 同入口 + nginx 反代到 home GPT-SoVITS。
    /// `POST` 协议：body `{"text": ...}` + Bearer token = 同 fuxi-im pair token。
    var ttsRemoteURL: String
    /// UI 形态：药丸（默认，老用户无感升级）/ 立绘（桌宠模式）。
    var uiMode: UIMode

    enum UIMode: String, CaseIterable, Identifiable, Codable {
        /// 现有禅意药丸——160×32 圆角胶囊悬浮 dock 上方。
        case capsule
        /// 立绘桌宠——~280×420 pose 图 panel，仙气线条。
        case pet
        var id: String { rawValue }
        var label: String {
            switch self {
            case .capsule: return "药丸"
            case .pet: return "立绘"
            }
        }
    }

    enum TTSProvider: String, CaseIterable, Identifiable, Codable {
        /// macOS AVSpeechSynthesizer 内置 zh-CN 音色（compact / enhanced / premium）。
        case system
        /// 远端 GPT-SoVITS / Bert-VITS2 等 voice cloning 服务（派蒙 / 钟离 等角色音）。
        case remote
        var id: String { rawValue }
        var label: String {
            switch self {
            case .system: return "系统语音（macOS）"
            case .remote: return "角色语音（远端）"
            }
        }
    }

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

    static let userDefaultsKey = "cn.qmledmq.fuxi.xuannv.settings"

    static let `default` = Settings(
        // 默认填家用部署公网地址——install-jarvis.sh 已把 token 写进 Keychain，
        // 用户开 App 即用，无需进设置面板填地址。
        // 想切本地 dev：设置面板覆盖即可。
        baseURL: "https://im.qmledmq.cn:8443",
        pairToken: "",
        triggerMode: .both,
        // wake 复用 im 子域——避免给 wake.qmledmq.cn 单独配 DDNS A 记录 + Clash
        // TUN 路由规则。nginx 在 im 站点加 /wake/ location 反代 :9101。
        wakeServerURL: "wss://im.qmledmq.cn:8443/wake/api/wake",
        wakeToken: "",
        wakeKeywords: ["玄女", "贾维斯"],
        picovoiceKey: "",
        // 默认 ⌃⌥M——避开 ⌘Space (Spotlight) / ⌥Space / ⌃Space 一族常见冲突。
        // m=0x2E。用户在设置里可改。
        hotkey: HotkeyCombo(modifiers: [.control, .option], keyCode: 0x2E),
        // 空串 = 自动选最高质量；用户在设置 → 语音 picker 里选完再覆盖。
        ttsVoice: "",
        ttsRate: 0.55,
        ttsProvider: .system,
        // 默认走家用入口同 fuxi-im 域 + nginx /api/tts 反代——避免再开端口/再签证书。
        ttsRemoteURL: "https://im.qmledmq.cn:8443/api/tts",
        // 默认走药丸——老用户升级无感；新用户进设置切立绘。
        uiMode: .capsule
    )

    static func load() -> Settings {
        let d = UserDefaults.standard
        // Keychain 路径 ad-hoc 签名下被 macOS 14+ ACL 拦截（每次重 sign 都换 hash =
        // 新 app identity，老 keychain ACL 不放行）。个人工具改走 UserDefaults
        // 直接持 token——install.sh `defaults write` 即可注入，App 一致 read。
        let dec = (d.data(forKey: Self.userDefaultsKey)).flatMap {
            try? JSONDecoder().decode(SettingsCodable.self, from: $0)
        }
        return Settings(
            baseURL: dec?.baseURL ?? Self.default.baseURL,
            pairToken: d.string(forKey: "pairToken") ?? "",
            triggerMode: dec.flatMap { TriggerMode(rawValue: $0.triggerMode) } ?? Self.default.triggerMode,
            wakeServerURL: dec?.wakeServerURL ?? Self.default.wakeServerURL,
            wakeToken: d.string(forKey: "wakeToken") ?? "",
            wakeKeywords: dec?.wakeKeywords ?? Self.default.wakeKeywords,
            picovoiceKey: d.string(forKey: "picovoiceKey") ?? "",
            hotkey: dec?.hotkey ?? Self.default.hotkey,
            // 老版本 ttsVoice 默认值是 "zh-CN"——它不是合法 identifier，运行时
            // `AVSpeechSynthesisVoice(identifier:)` 返 nil 走 language fallback；
            // 但 v0.2 Synthesizer 把空串当"自动选最高质量"信号，所以把老 "zh-CN"
            // 也归一为 ""，让升级用户立即享受 premium 音色。
            ttsVoice: {
                let v = dec?.ttsVoice ?? Self.default.ttsVoice
                return v == "zh-CN" ? "" : v
            }(),
            ttsRate: dec?.ttsRate ?? Self.default.ttsRate,
            ttsProvider: dec.flatMap { TTSProvider(rawValue: $0.ttsProvider ?? "") } ?? Self.default.ttsProvider,
            ttsRemoteURL: dec?.ttsRemoteURL ?? Self.default.ttsRemoteURL,
            // 老 UserDefaults 没 uiMode → 回 capsule，无感
            uiMode: dec.flatMap { UIMode(rawValue: $0.uiMode ?? "") } ?? Self.default.uiMode
        )
    }

    func save() {
        let enc = SettingsCodable(
            baseURL: baseURL,
            triggerMode: triggerMode.rawValue,
            wakeServerURL: wakeServerURL,
            wakeKeywords: wakeKeywords,
            hotkey: hotkey,
            ttsVoice: ttsVoice,
            ttsRate: ttsRate,
            ttsProvider: ttsProvider.rawValue,
            ttsRemoteURL: ttsRemoteURL,
            uiMode: uiMode.rawValue
        )
        let d = UserDefaults.standard
        if let data = try? JSONEncoder().encode(enc) {
            d.set(data, forKey: Self.userDefaultsKey)
        }
        d.set(pairToken, forKey: "pairToken")
        d.set(wakeToken, forKey: "wakeToken")
        d.set(picovoiceKey, forKey: "picovoiceKey")
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
    var ttsRate: Double?
    var ttsProvider: String?
    var ttsRemoteURL: String?
    var uiMode: String?
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
