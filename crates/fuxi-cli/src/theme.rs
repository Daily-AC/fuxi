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

/// 伏羲 TUI 调色板。
///
/// 26 色 palette 全按 Catppuccin 官方命名（base/mantle/crust/text/...），
/// 不在此层做语义转换——语义 alias（如 `focus_border`）在 inherent impl 里提供。
/// 这样做的好处：新增一款主题只需填 26 个 hex，语义层零改动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    // 背景三级
    pub base: Color,   // 主背景
    pub mantle: Color, // 次级背景
    pub crust: Color,  // 第三级背景（最深）

    // 文本三级
    pub text: Color, // 正文
    pub subtext1: Color,
    pub subtext0: Color,

    // overlay / surface 各三级（灰阶层次）
    pub overlay2: Color,
    pub overlay1: Color,
    pub overlay0: Color,
    pub surface2: Color,
    pub surface1: Color,
    pub surface0: Color,

    // accent 14 色
    pub blue: Color,
    pub lavender: Color,
    pub sapphire: Color,
    pub sky: Color,
    pub teal: Color,
    pub green: Color,
    pub yellow: Color,
    pub peach: Color,
    pub maroon: Color,
    pub red: Color,
    pub mauve: Color,
    pub pink: Color,
    pub flamingo: Color,
    pub rosewater: Color,
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
    pub fn user_message(&self) -> Color {
        self.teal
    }

    /// 玄女 / 门客消息色（紫，与 focus_border 同源 = 品牌色）。
    pub fn agent_message(&self) -> Color {
        self.mauve
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

#[cfg(test)]
mod tests {
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
}
