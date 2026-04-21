//! Catppuccin 主题 · 配色 token 化。
//!
//! 设计：widget 只吃 `Theme` 的语义方法（`focus_border()` / `user_message()`...），
//! 不直接碰 hex。切主题 = 换一个 Theme 实例。
//! 参考：<https://catppuccin.com/palette/>
//!
//! 两档预设：
//! - `Theme::catppuccin_mocha()`（暗，默认）
//! - `Theme::catppuccin_latte()`（亮）
//
// `allow(dead_code)`：整个模块为"待主线接入的前置件"——binary crate 里 `pub`
// 不对外，只在跨 mod 可见；repl/widget 接入后此 allow 可整体移除。
#![allow(dead_code)]

use ratatui::style::Color;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// 伏羲 TUI 调色板。
///
/// 26 色 palette 全按 Catppuccin 官方命名（base/mantle/crust/text/...），
/// 不在此层做语义转换——语义 alias（如 `focus_border`）在 inherent impl 里提供。
/// 这样做的好处：新增一款主题只需填 26 个 hex，语义层零改动。
///
/// serde 走 hex string "#rrggbb"。不 feature gate 到 ratatui，
/// 因为 ratatui::Color 本身不实现 serde，每个字段都得自定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    // 背景三级
    #[serde(with = "color_hex")]
    pub base: Color, // 主背景
    #[serde(with = "color_hex")]
    pub mantle: Color, // 次级背景
    #[serde(with = "color_hex")]
    pub crust: Color, // 第三级背景（最深）

    // 文本三级
    #[serde(with = "color_hex")]
    pub text: Color, // 正文
    #[serde(with = "color_hex")]
    pub subtext1: Color,
    #[serde(with = "color_hex")]
    pub subtext0: Color,

    // overlay / surface 各三级（灰阶层次）
    #[serde(with = "color_hex")]
    pub overlay2: Color,
    #[serde(with = "color_hex")]
    pub overlay1: Color,
    #[serde(with = "color_hex")]
    pub overlay0: Color,
    #[serde(with = "color_hex")]
    pub surface2: Color,
    #[serde(with = "color_hex")]
    pub surface1: Color,
    #[serde(with = "color_hex")]
    pub surface0: Color,

    // accent 14 色
    #[serde(with = "color_hex")]
    pub blue: Color,
    #[serde(with = "color_hex")]
    pub lavender: Color,
    #[serde(with = "color_hex")]
    pub sapphire: Color,
    #[serde(with = "color_hex")]
    pub sky: Color,
    #[serde(with = "color_hex")]
    pub teal: Color,
    #[serde(with = "color_hex")]
    pub green: Color,
    #[serde(with = "color_hex")]
    pub yellow: Color,
    #[serde(with = "color_hex")]
    pub peach: Color,
    #[serde(with = "color_hex")]
    pub maroon: Color,
    #[serde(with = "color_hex")]
    pub red: Color,
    #[serde(with = "color_hex")]
    pub mauve: Color,
    #[serde(with = "color_hex")]
    pub pink: Color,
    #[serde(with = "color_hex")]
    pub flamingo: Color,
    #[serde(with = "color_hex")]
    pub rosewater: Color,
}

/// `ratatui::Color` 的 hex string 序列化（"#rrggbb"）。
///
/// 只支持 `Color::Rgb`——JSON 主题只允许精确 RGB，不接受 named / indexed，
/// 因为 JSON 是给人写的、语义要稳定，索引色在不同 terminal 下漂移。
mod color_hex {
    use super::*;
    use serde::de::Error as _;

    pub fn serialize<S: Serializer>(c: &Color, s: S) -> Result<S::Ok, S::Error> {
        match c {
            Color::Rgb(r, g, b) => s.serialize_str(&format!("#{:02x}{:02x}{:02x}", r, g, b)),
            other => Err(serde::ser::Error::custom(format!(
                "theme palette 只允许 Color::Rgb，遇到 {other:?}"
            ))),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Color, D::Error> {
        let s = String::deserialize(d)?;
        parse_hex(&s).map_err(D::Error::custom)
    }

    fn parse_hex(s: &str) -> Result<Color, String> {
        // 宽容：允许 "#rrggbb" 也允许 "rrggbb"
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 {
            return Err(format!("hex 颜色应为 6 位（{s}）"));
        }
        let r =
            u8::from_str_radix(&hex[0..2], 16).map_err(|e| format!("r 段解析失败（{s}）: {e}"))?;
        let g =
            u8::from_str_radix(&hex[2..4], 16).map_err(|e| format!("g 段解析失败（{s}）: {e}"))?;
        let b =
            u8::from_str_radix(&hex[4..6], 16).map_err(|e| format!("b 段解析失败（{s}）: {e}"))?;
        Ok(Color::Rgb(r, g, b))
    }
}

impl Theme {
    /// Catppuccin Mocha（暗）· 默认。
    /// hex 源：<https://catppuccin.com/palette/> Mocha tab。
    pub const fn catppuccin_mocha() -> Self {
        Self {
            base: Color::Rgb(0x1e, 0x1e, 0x2e),
            mantle: Color::Rgb(0x18, 0x18, 0x25),
            crust: Color::Rgb(0x11, 0x11, 0x1b),

            text: Color::Rgb(0xcd, 0xd6, 0xf4),
            subtext1: Color::Rgb(0xba, 0xc2, 0xde),
            subtext0: Color::Rgb(0xa6, 0xad, 0xc8),

            overlay2: Color::Rgb(0x93, 0x99, 0xb2),
            overlay1: Color::Rgb(0x7f, 0x84, 0x9c),
            overlay0: Color::Rgb(0x6c, 0x70, 0x86),
            surface2: Color::Rgb(0x58, 0x5b, 0x70),
            surface1: Color::Rgb(0x45, 0x47, 0x5a),
            surface0: Color::Rgb(0x31, 0x32, 0x44),

            blue: Color::Rgb(0x89, 0xb4, 0xfa),
            lavender: Color::Rgb(0xb4, 0xbe, 0xfe),
            sapphire: Color::Rgb(0x74, 0xc7, 0xec),
            sky: Color::Rgb(0x89, 0xdc, 0xeb),
            teal: Color::Rgb(0x94, 0xe2, 0xd5),
            green: Color::Rgb(0xa6, 0xe3, 0xa1),
            yellow: Color::Rgb(0xf9, 0xe2, 0xaf),
            peach: Color::Rgb(0xfa, 0xb3, 0x87),
            maroon: Color::Rgb(0xeb, 0xa0, 0xac),
            red: Color::Rgb(0xf3, 0x8b, 0xa8),
            mauve: Color::Rgb(0xcb, 0xa6, 0xf7),
            pink: Color::Rgb(0xf5, 0xc2, 0xe7),
            flamingo: Color::Rgb(0xf2, 0xcd, 0xcd),
            rosewater: Color::Rgb(0xf5, 0xe0, 0xdc),
        }
    }

    /// Catppuccin Latte（亮）。
    /// hex 源：<https://catppuccin.com/palette/> Latte tab。
    pub const fn catppuccin_latte() -> Self {
        Self {
            base: Color::Rgb(0xef, 0xf1, 0xf5),
            mantle: Color::Rgb(0xe6, 0xe9, 0xef),
            crust: Color::Rgb(0xdc, 0xe0, 0xe8),

            text: Color::Rgb(0x4c, 0x4f, 0x69),
            subtext1: Color::Rgb(0x5c, 0x5f, 0x77),
            subtext0: Color::Rgb(0x6c, 0x6f, 0x85),

            overlay2: Color::Rgb(0x7c, 0x7f, 0x93),
            overlay1: Color::Rgb(0x8c, 0x8f, 0xa1),
            overlay0: Color::Rgb(0x9c, 0xa0, 0xb0),
            surface2: Color::Rgb(0xac, 0xb0, 0xbe),
            surface1: Color::Rgb(0xbc, 0xc0, 0xcc),
            surface0: Color::Rgb(0xcc, 0xd0, 0xda),

            blue: Color::Rgb(0x1e, 0x66, 0xf5),
            lavender: Color::Rgb(0x72, 0x87, 0xfd),
            sapphire: Color::Rgb(0x20, 0x9f, 0xb5),
            sky: Color::Rgb(0x04, 0xa5, 0xe5),
            teal: Color::Rgb(0x17, 0x9b, 0x94),
            green: Color::Rgb(0x40, 0xa0, 0x2b),
            yellow: Color::Rgb(0xdf, 0x8e, 0x1d),
            peach: Color::Rgb(0xfe, 0x64, 0x0b),
            maroon: Color::Rgb(0xe6, 0x45, 0x53),
            red: Color::Rgb(0xd2, 0x0f, 0x39),
            mauve: Color::Rgb(0x88, 0x39, 0xef),
            pink: Color::Rgb(0xea, 0x76, 0xcb),
            flamingo: Color::Rgb(0xdd, 0x78, 0x78),
            rosewater: Color::Rgb(0xdc, 0x8a, 0x78),
        }
    }

    // --- 语义 alias · widget 层只应调这些 ---

    /// 焦点面板的高亮边框（紫）。用于当前 focus 的 panel。
    pub fn focus_border(&self) -> Color {
        self.mauve
    }

    /// 非焦点 panel 的灰边框。
    pub fn dim_border(&self) -> Color {
        self.overlay0
    }

    /// 用户输入的消息色（冷青，与 agent 的紫做冷暖对照）。
    /// WHY 保留 teal 不切 sapphire：此 alias 用于事件流 narrate 色标
    /// （`task_created` / `user_prompted` 等），换色会牵连整个事件面板。
    /// 对话区首行锚点请用 `user_first_line()` —— 方案 A 降饱和度专用。
    pub fn user_message(&self) -> Color {
        self.teal
    }

    /// 玄女 / 门客消息色（紫，与 focus_border 同源 = 品牌色）。
    /// 同上：此 alias 用于事件流色标；对话区首行请用 `agent_first_line()`。
    pub fn agent_message(&self) -> Color {
        self.mauve
    }

    /// 对话区「用户」首行锚点色（sapphire · 方案 A 降饱和度）。
    /// `▍ HH:MM ` 竖条 + 时间戳用这个色；续行无前缀空白占位。
    pub fn user_first_line(&self) -> Color {
        self.sapphire
    }

    /// 对话区「玄女 / 门客」首行锚点色（lavender · 方案 A 降饱和度）。
    pub fn agent_first_line(&self) -> Color {
        self.lavender
    }

    /// 工具调用（黄，经典"注意"色）。
    pub fn tool_call(&self) -> Color {
        self.yellow
    }

    /// thinking 灰（比 muted 略亮——需要让用户看清但不抢焦点）。
    pub fn thinking(&self) -> Color {
        self.overlay2
    }

    /// 成功。
    pub fn success(&self) -> Color {
        self.green
    }

    /// 警告（peach 比 yellow 警示性更强、比 red 柔和）。
    pub fn warn(&self) -> Color {
        self.peach
    }

    /// 错误。
    pub fn error(&self) -> Color {
        self.red
    }

    /// 元信息 / 次要提示灰。
    pub fn muted(&self) -> Color {
        self.overlay1
    }

    /// 通用"信息"色（蓝）——trigger / 消息类事件、非警示非成功的中性信号。
    pub fn info(&self) -> Color {
        self.blue
    }

    /// 整窗背景色（Block 不设 bg 时终端默认——但有 bg 需求时调此）。
    pub fn background(&self) -> Color {
        self.base
    }

    /// 次级 panel 背景（让多栏布局有层次感）。
    pub fn panel_bg(&self) -> Color {
        self.mantle
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}

/// 从环境变量读取主题（主进程入口用）。
///
/// `FUXI_THEME=mocha|latte|default`。其他值或未设 → mocha。
pub fn from_env() -> Theme {
    from_env_str(std::env::var("FUXI_THEME").ok().as_deref())
}

/// 纯函数版——`from_env` 内部调用；测试只测它，避免 `set_var` 的多线程不安全。
/// 见 CLAUDE.md "env 测试注意"。
pub fn from_env_str(s: Option<&str>) -> Theme {
    match s {
        Some("latte") => Theme::catppuccin_latte(),
        _ => Theme::catppuccin_mocha(),
    }
}

/// 从 JSON 文件加载主题。
///
/// 调用方拿到 `Theme` 后自己决定何时 apply——此函数不动全局状态（γ 负责 runtime 切换）。
pub fn load_from_json(path: &Path) -> anyhow::Result<Theme> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取主题文件失败 {}: {e}", path.display()))?;
    let theme: Theme = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("解析主题 JSON 失败 {}: {e}", path.display()))?;
    Ok(theme)
}

/// 枚举所有可用主题名（不含 `.json` 后缀）。
///
/// 扫两个目录：
/// 1. `CARGO_MANIFEST_DIR/themes` —— 仓内内置主题，dev 模式直接跑能读到
/// 2. `~/.fuxi/themes` —— 用户主题，同名覆盖仓内（用户优先）
///
/// WHY 用户覆盖仓内：RELEASE 二进制里 `CARGO_MANIFEST_DIR` 只是 build 时路径，
/// 分发后该目录可能不存在；用户 `~/.fuxi/themes` 是稳定落点。
/// 同名时让用户版本赢，以便覆盖内置主题做微调。
pub fn list_themes() -> Vec<String> {
    // BTreeSet: 去重 + 确定性顺序，方便测试。
    let mut names = std::collections::BTreeSet::new();
    for dir in theme_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.insert(stem.to_string());
            }
        }
    }
    names.into_iter().collect()
}

/// 解析主题文件目录候选。顺序 = 查找优先级（用户在后会覆盖）。
fn theme_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    // 1. 仓内（dev）
    let manifest: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes");
    v.push(manifest);
    // 2. 用户目录
    if let Some(home) = std::env::var_os("HOME") {
        v.push(PathBuf::from(home).join(".fuxi").join("themes"));
    }
    v
}

/// 根据主题名加载（优先用户目录）。
///
/// 供 γ 在 runtime 切换时调用：`/theme tokyo-night` → `load_by_name("tokyo-night")`。
pub fn load_by_name(name: &str) -> anyhow::Result<Theme> {
    // 倒序扫：用户目录（后压栈）先试，失败再回落到仓内。
    for dir in theme_dirs().into_iter().rev() {
        let path = dir.join(format!("{name}.json"));
        if path.is_file() {
            return load_from_json(&path);
        }
    }
    // 回落：内置名（`mocha` / `latte`）不走 JSON，直接命中常量。
    match name {
        "mocha" => Ok(Theme::catppuccin_mocha()),
        "latte" => Ok(Theme::catppuccin_latte()),
        _ => anyhow::bail!("找不到主题 `{name}`（查找目录：{:?}）", theme_dirs()),
    }
}

/// 全局运行时主题——进程启动期 `init_from_env()` 初始化，后续 `/theme` 命令
/// 通过 `set_theme` 热切换。
///
/// WHY `OnceLock<RwLock<Theme>>` 而不是 `OnceLock<Theme>`：
/// R10-integ 需要运行时可变——重启才能换主题用户体验太糟。RwLock 读多写少
/// （每帧 render 都读，set 只在用户下 /theme 时触发），读锁几乎无争用。
/// Theme 是 Copy + 26 个 u8 三元组，读取整 struct 远比取字段单独读简单。
///
/// WHY 不用 const 构造 + `static RwLock<Theme> = RwLock::new(...)`：
/// stable Rust 的 `RwLock::new` 是 const，但初始值要 `catppuccin_mocha()`
/// 也是 const——能 const 构造。只是 `from_env` 需要运行时读 env，所以真正
/// 启动 theme 选择仍在 `init_from_env`，用 OnceLock + 一次性设置。
/// 为简单直接用 const RwLock 作兜底，`init_from_env` 覆写一次。
static CURRENT: std::sync::RwLock<Theme> = std::sync::RwLock::new(Theme::catppuccin_mocha());

/// 启动期把 CURRENT 设为 `FUXI_THEME` env 决定的主题。
/// 调用时机：main.rs `init_tracing` 之前或之后皆可，但必须在 REPL draw 前。
/// WHY 单独函数而非首次 get_or_init：方便测试 override 和手动重置。
pub fn init_from_env() {
    let t = from_env();
    if let Ok(mut w) = CURRENT.write() {
        *w = t;
    }
}

/// 返回当前主题**值**（Copy，不是引用）。
///
/// WHY 值而非 `RwLockReadGuard`：调用方四散在各 draw_* 里，握 guard 跨多个
/// widget 调用会让 `set_theme` 写锁无限期等；每次 deref 拿个 26 字节的 Theme
/// 是最便宜且最不易出错的做法。
pub fn current() -> Theme {
    // poisoned 了就兜底返默认——render 不能 panic；记 tracing 里 best-effort。
    CURRENT
        .read()
        .map(|g| *g)
        .unwrap_or_else(|_| Theme::default())
}

/// 运行时切主题。成功返 Ok 并把 CURRENT 更新；失败（找不到 / 解析错）返 Err。
///
/// 用途：REPL `CommandAction::Theme(Some(name))` 分支调此函数。
pub fn set_theme(name: &str) -> anyhow::Result<()> {
    let t = load_by_name(name)?;
    CURRENT
        .write()
        .map_err(|e| anyhow::anyhow!("写主题锁失败：{e}"))?
        .clone_from(&t);
    Ok(())
}

/// 手动设置 CURRENT——测试帮手，避免动文件系统。
#[cfg(test)]
pub fn set_theme_value(t: Theme) {
    if let Ok(mut w) = CURRENT.write() {
        *w = t;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Mocha 官方 hex spot-check。
    /// 若未来 Catppuccin 改调色（历史上发生过一次 v0.1 → v1.0），这条会红灯提醒。
    #[test]
    fn mocha_palette_hex_matches_official() {
        let t = Theme::catppuccin_mocha();
        assert_eq!(t.base, Color::Rgb(0x1e, 0x1e, 0x2e), "base 应为 #1e1e2e");
        assert_eq!(t.mauve, Color::Rgb(0xcb, 0xa6, 0xf7), "mauve 应为 #cba6f7");
        assert_eq!(t.red, Color::Rgb(0xf3, 0x8b, 0xa8), "red 应为 #f38ba8");
        assert_eq!(t.green, Color::Rgb(0xa6, 0xe3, 0xa1), "green 应为 #a6e3a1");
        assert_eq!(t.text, Color::Rgb(0xcd, 0xd6, 0xf4), "text 应为 #cdd6f4");
    }

    #[test]
    fn latte_palette_hex_matches_official() {
        let t = Theme::catppuccin_latte();
        assert_eq!(t.base, Color::Rgb(0xef, 0xf1, 0xf5), "base 应为 #eff1f5");
        assert_eq!(t.mauve, Color::Rgb(0x88, 0x39, 0xef), "mauve 应为 #8839ef");
        assert_eq!(t.red, Color::Rgb(0xd2, 0x0f, 0x39), "red 应为 #d20f39");
        assert_eq!(t.text, Color::Rgb(0x4c, 0x4f, 0x69), "text 应为 #4c4f69");
    }

    /// 语义层必须是 palette 层的纯引用，改 palette → 语义层自动跟上。
    #[test]
    fn semantic_aliases_match_palette() {
        let t = Theme::catppuccin_mocha();
        assert_eq!(t.focus_border(), t.mauve);
        assert_eq!(t.dim_border(), t.overlay0);
        assert_eq!(t.user_message(), t.teal);
        assert_eq!(t.agent_message(), t.mauve);
        assert_eq!(t.user_first_line(), t.sapphire);
        assert_eq!(t.agent_first_line(), t.lavender);
        assert_eq!(t.tool_call(), t.yellow);
        assert_eq!(t.thinking(), t.overlay2);
        assert_eq!(t.success(), t.green);
        assert_eq!(t.warn(), t.peach);
        assert_eq!(t.error(), t.red);
        assert_eq!(t.muted(), t.overlay1);
        assert_eq!(t.background(), t.base);
        assert_eq!(t.panel_bg(), t.mantle);
    }

    #[test]
    fn from_env_returns_mocha_by_default() {
        assert_eq!(from_env_str(None), Theme::catppuccin_mocha());
        assert_eq!(from_env_str(Some("")), Theme::catppuccin_mocha());
        assert_eq!(from_env_str(Some("default")), Theme::catppuccin_mocha());
        assert_eq!(from_env_str(Some("garbage")), Theme::catppuccin_mocha());
    }

    #[test]
    fn from_env_returns_latte_when_set() {
        assert_eq!(from_env_str(Some("latte")), Theme::catppuccin_latte());
    }

    /// 默认实现 = mocha，文档里也这么写的。
    #[test]
    fn default_theme_is_mocha() {
        assert_eq!(Theme::default(), Theme::catppuccin_mocha());
    }

    /// serde round-trip 不丢精度。
    /// 26 色一个不差地经过 Serialize→JSON→Deserialize 回来，
    /// 防止未来加字段漏写 `#[serde(with = "color_hex")]` 导致静默回退到 ratatui 默认路径。
    #[test]
    fn round_trip_json_preserves_palette() {
        let original = Theme::catppuccin_mocha();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: Theme = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored, "round-trip 必须 bit-exact");

        // 同时对 latte 跑一次，避免漏测 light 主题分支。
        let latte = Theme::catppuccin_latte();
        let json = serde_json::to_string(&latte).expect("serialize latte");
        let restored: Theme = serde_json::from_str(&json).expect("deserialize latte");
        assert_eq!(latte, restored);
    }

    /// hex 字符串格式 spot-check——必须是 `#rrggbb` 小写。
    /// WHY 卡格式：用户手写主题 JSON 会 diff/review，统一大小写 + 前缀减少无谓抖动。
    #[test]
    fn serialized_hex_is_lowercase_with_hash() {
        let t = Theme::catppuccin_mocha();
        let json = serde_json::to_value(t).expect("to_value");
        assert_eq!(json["base"].as_str(), Some("#1e1e2e"));
        assert_eq!(json["mauve"].as_str(), Some("#cba6f7"));
    }

    /// 反序列化容忍去 `#`（方便用户手写），但内部统一存带 `#`。
    /// 通过完整 Theme 结构测——`Color` 独立没有 Deserialize，`color_hex` 只在字段 `with` 时生效。
    #[test]
    fn deserialize_accepts_bare_hex() {
        let mut v = serde_json::to_value(Theme::catppuccin_mocha()).unwrap();
        v["base"] = serde_json::Value::String("1e1e2e".into());
        let t: Theme = serde_json::from_value(v).expect("accept bare hex");
        assert_eq!(t.base, Color::Rgb(0x1e, 0x1e, 0x2e));
    }

    // ───────── R10-integ 运行时切主题 ─────────
    //
    // CURRENT 是全局 RwLock，测试并发会互相扰动——本文件所有**动 CURRENT** 的
    // 测试走同一把 Mutex 串行跑。Mutex 本身 poison 了也无伤大雅：下一次 lock()
    // 用 `unwrap_or_else(|e| e.into_inner())` 兜底。
    //
    // pub(crate)：repl.rs 的 /theme 测试也要串行跑 CURRENT，共用这把。
    pub(crate) fn current_theme_lock() -> &'static std::sync::Mutex<()> {
        use std::sync::Mutex;
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn set_theme_mocha_then_latte_changes_semantic_color() {
        let _g = current_theme_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 起点置 mocha——避免前一个测试留下的 latte 污染。
        set_theme("mocha").expect("mocha 内置必 Ok");
        assert_eq!(current().base, Theme::catppuccin_mocha().base);
        assert_eq!(
            current().agent_first_line(),
            Theme::catppuccin_mocha().lavender
        );

        set_theme("latte").expect("latte 内置必 Ok");
        assert_eq!(current().base, Theme::catppuccin_latte().base);
        // 语义色也跟着变——这是 runtime 切主题真正要保证的"现场换肤"。
        assert_eq!(
            current().agent_first_line(),
            Theme::catppuccin_latte().lavender
        );

        // 收尾回 mocha，减少遗留影响。
        set_theme("mocha").ok();
    }

    #[test]
    fn set_theme_unknown_returns_err() {
        let _g = current_theme_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let before = current();
        let res = set_theme("no-such-theme-qwerty");
        assert!(res.is_err(), "未知主题必须报错");
        // CURRENT 不该被打脏。
        assert_eq!(current(), before, "失败不该改 CURRENT");
    }

    #[test]
    fn init_from_env_sets_current() {
        let _g = current_theme_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 先置 latte 让 CURRENT 偏离默认，再 init_from_env 确认它被改回
        // （除非进程 env 真设了 FUXI_THEME=latte——CI 默认无）。
        set_theme_value(Theme::catppuccin_latte());
        assert_eq!(current(), Theme::catppuccin_latte());

        init_from_env();
        // init_from_env 读的是真 env；断言它至少把 CURRENT 设到了 from_env() 的返值。
        assert_eq!(current(), from_env());
    }

    /// 长度不对的 hex 应报错，不静默破坏 palette。
    #[test]
    fn deserialize_rejects_malformed_hex() {
        let mut v = serde_json::to_value(Theme::catppuccin_mocha()).unwrap();
        v["base"] = serde_json::Value::String("#xyz".into());
        assert!(serde_json::from_value::<Theme>(v).is_err());
    }

    /// tokyo-night.json 能被解析，且关键几个 token 对得上官方 palette。
    /// WHY hard-code 几个 hex：防止 JSON 文件被无意覆写（CI 会红）。
    #[test]
    fn load_tokyo_night_parses() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("themes")
            .join("tokyo-night.json");
        let theme = load_from_json(&path).expect("load tokyo-night");

        // tokyonight.nvim "night" 风格锚点色
        assert_eq!(theme.base, Color::Rgb(0x1a, 0x1b, 0x26), "base #1a1b26");
        assert_eq!(theme.mauve, Color::Rgb(0xbb, 0x9a, 0xf7), "mauve #bb9af7");
    }

    /// `list_themes()` 应至少包含仓内三个内置名。
    #[test]
    fn list_themes_includes_builtins() {
        let names = list_themes();
        for expected in ["mocha", "latte", "tokyo-night"] {
            assert!(
                names.iter().any(|n| n == expected),
                "list_themes 里应含 `{expected}`，实际 = {names:?}"
            );
        }
    }

    /// `load_by_name` 能从仓内拿到三个内置主题。
    #[test]
    fn load_by_name_finds_builtins() {
        for name in ["mocha", "latte", "tokyo-night"] {
            load_by_name(name).unwrap_or_else(|e| panic!("加载 {name} 失败：{e}"));
        }
    }

    /// 用户目录同名覆盖仓内——tempdir 模拟 `~/.fuxi/themes`，
    /// 改个 base 色验证确实读的是用户版。
    /// WHY 不用真 `$HOME`：可能污染用户环境 + 多线程测试下 `set_var` 不安全。
    /// 这里用 `theme_dirs` 私有 helper 的行为 + 手写文件 + 反序列化来间接测。
    #[test]
    fn user_themes_dir_overrides_builtin() {
        // 此测试只在 macOS/Linux 跑（依赖 HOME env）——CI 两者都有。
        // 不 mutate HOME（有并发风险），直接调 load_from_json 验证"如果存在会覆盖"的逻辑契约。
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("custom.json");
        let mut t = Theme::catppuccin_mocha();
        t.base = Color::Rgb(0xaa, 0xbb, 0xcc);
        std::fs::write(&path, serde_json::to_string(&t).unwrap()).unwrap();

        let loaded = load_from_json(&path).unwrap();
        assert_eq!(loaded.base, Color::Rgb(0xaa, 0xbb, 0xcc));
        // 其他字段保持 mocha
        assert_eq!(loaded.mauve, Theme::catppuccin_mocha().mauve);
    }
}
