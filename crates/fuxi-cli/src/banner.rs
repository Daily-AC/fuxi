//! 启动 banner · 太极八卦（seedream→chafa braille）+ 伏羲标题 + 渐变上下框。
//!
//! 字符阵流程（离线生成，结果嵌为 const）：
//! 1. `seedream` 生一张「纯黑线八卦 + 太极」PNG
//! 2. 二值化 + 反相 + 缩放到 600px
//! 3. `chafa --symbols=braille` 转 48×20 字符阵
//! 4. 去色只取字符，banner 渲染时从 `theme.lavender` 上色
//!
//! WHY 离线生成：运行时没依赖，二进制不带 chafa / PIL 的跑版成本。
//! 后续换主题色只改 Rust 侧上色，不用重跑图→字符管线。

use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// 太极八卦字符阵 · 48 col × 20 row · 纯 braille。
///
/// 先天八卦方位：乾☰上 / 坤☷下 / 离☲右 / 坎☵左 / 兑☱右上 /
/// 震☳右下 / 巽☴左上 / 艮☶左下。中央为阴阳双鱼 + 鱼眼。
const BAGUA: [&str; 20] = [
    "                    ⠿⠿⠿⠇⠸⠿⠿⠿                    ",
    "            ⣠⡀      ⣛⣛⡛⡃⢘⣛⣛⣛      ⢀⣄            ",
    "          ⢠⡾⢋⣴⠆⡀    ⠉⠉ ⠁⠈⠉⠉⠉     ⠰⣦⡙⢷⡄          ",
    "        ⣠⡾⢂⡐⠟⣡⡾⠋ ⣠⣤⣶⣾⣿⠿⠛⠛⠛⠻⠷⢶⣤⣀ ⠙⢷⣌⠻⢂⡐⢷⣄        ",
    "       ⠘⠋⣴⠟⣡⡦⠉⣀⣴⣿⣿⣿⡿⠋         ⠙⠻⣦⣀⠉⢴⣌⠻⣦⡙⠃       ",
    "          ⠺⠋⢀⣼⣿⣿⣿⣿⣿⠁  ⢀⣤⣤⡀       ⠹⣧⡀⠙⠷          ",
    "           ⢠⣿⣿⣿⣿⣿⣿⡇   ⣿⣿⣿⣿        ⠈⢿⡄           ",
    "    ⣀     ⢠⣿⣿⣿⣿⣿⣿⣿⣷   ⠙⠛⠛⠋          ⢿⡄     ⣀    ",
    "    ⣿ ⣿⢸⣇ ⣼⣿⣿⣿⣿⣿⣿⣿⣿⣧⡀               ⠸⣧ ⢸⡇⣿ ⣿    ",
    "    ⠛ ⠛⠘⠛ ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣶⣤⣀⡀            ⣿ ⠘⠃⠛ ⠛    ",
    "    ⣿ ⣿⢸⣇ ⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦⣄         ⣿ ⢸⡇⣿ ⣿    ",
    "    ⠿ ⠿⠘⠛ ⢻⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣦       ⢸⡟ ⠘⠃⠿ ⠿    ",
    "          ⠈⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠟⠉⠉⠻⣿⣿⣿⡇     ⢀⣿⠁          ",
    "           ⠘⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡀  ⢀⣿⣿⣿⡇    ⣠⡿⠁           ",
    "        ⢀⣄⠻⣦⡀⠻⣿⣿⣿⣿⣿⣿⣿⣿⣿⣶⣶⣿⣿⣿⡿⠁  ⢀⣴⠟⢀⣴⠟⣠⡀        ",
    "       ⠺⣦⡙⢷⠌⠡⣦⡈⠙⢿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠟⠁⣀⣤⡾⠋⢁⣴⠌⠡⡾⢋⣴⠗       ",
    "        ⠈⠛⣠⡘⢷⣌⠻⠆ ⠈⠙⠛⠿⠿⢿⣿⣿⡿⠶⠾⠛⠋⠁ ⢰⠟⣡⡾⢃⣄⠛⠁        ",
    "          ⠈⠻⣦⡙⠃     ⣤⣤⣤⡄⢠⣤⣤⣤     ⠘⢋⣴⠟⠁          ",
    "            ⠈       ⣴⣶⣶⡆⢰⣶⣶⣦       ⠁            ",
    "                    ⠶⠶⠶⠆⠰⠶⠶⠶                    ",
];

/// banner 视觉总宽度（列数）。用于居中其他元素。
const BANNER_WIDTH: usize = 48;

/// banner 总 `Line` 序列。
pub fn render(theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // ── 顶部 · 乾 ☰ 三阳爻：solid 实线 + 渐变色 ──
    for _ in 0..3 {
        lines.push(gradient_bar("━", BANNER_WIDTH, theme.sapphire, theme.mauve));
    }
    lines.push(Line::from(""));

    // ── 太极八卦字符阵 ──
    let bagua_style = Style::default().fg(theme.lavender);
    for row in BAGUA.iter() {
        lines.push(Line::from(vec![Span::styled(
            (*row).to_string(),
            bagua_style,
        )]));
    }
    lines.push(Line::from(""));

    // ── 标题行：伏 羲 · FUXI · v0.1.0 ──
    let title_line = Line::from(vec![
        Span::styled(
            "伏  羲",
            Style::default()
                .fg(theme.sapphire)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled(
            "F U X I",
            Style::default()
                .fg(theme.lavender)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled("v0.1.0", Style::default().fg(theme.overlay2)),
    ]);
    lines.push(center_line(title_line, BANNER_WIDTH));

    // ── 题词两行 ──
    lines.push(Line::from(""));
    lines.push(center_text(
        "通神明之德 · 类万物之情",
        Style::default()
            .fg(theme.subtext0)
            .add_modifier(Modifier::ITALIC),
        BANNER_WIDTH,
    ));
    lines.push(center_text(
        "玄女调度 · 门客效命",
        Style::default().fg(theme.overlay2),
        BANNER_WIDTH,
    ));
    lines.push(Line::from(""));

    // ── 底部 · 坤 ☷ 三阴爻：断线 + 反向渐变 ──
    for _ in 0..3 {
        lines.push(gradient_bar(
            "━━━  ",
            BANNER_WIDTH,
            theme.mauve,
            theme.sapphire,
        ));
    }

    lines
}

/// 用 `unit` 串填充到 `width` 列，并把每列 char 按位置 lerp `start→end` 颜色。
fn gradient_bar(unit: &str, width: usize, start: Color, end: Color) -> Line<'static> {
    // 先按 unit 填出一整条字符串到指定宽度（按显示宽度算）
    let mut buf = String::new();
    while unicode_width::UnicodeWidthStr::width(buf.as_str()) < width {
        buf.push_str(unit);
    }
    gradient_text(buf.as_str(), start, end)
}

fn gradient_text(text: &str, start: Color, end: Color) -> Line<'static> {
    let (sr, sg, sb) = rgb_of(start);
    let (er, eg, eb) = rgb_of(end);
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(chars.len());
    for (i, c) in chars.iter().enumerate() {
        let t = i as f32 / (n - 1).max(1) as f32;
        let r = lerp(sr, er, t);
        let g = lerp(sg, eg, t);
        let b = lerp(sb, eb, t);
        spans.push(Span::styled(
            c.to_string(),
            Style::default().fg(Color::Rgb(r, g, b)),
        ));
    }
    Line::from(spans)
}

fn rgb_of(c: Color) -> (u8, u8, u8) {
    if let Color::Rgb(r, g, b) = c {
        (r, g, b)
    } else {
        (200, 200, 200)
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// 居中纯文本到 `width` 列。
fn center_text(text: &str, style: Style, width: usize) -> Line<'static> {
    let w = unicode_width::UnicodeWidthStr::width(text);
    let pad = width.saturating_sub(w) / 2;
    Line::from(vec![
        Span::raw(" ".repeat(pad)),
        Span::styled(text.to_string(), style),
    ])
}

/// 居中一条既有 `Line`——算它的总显示宽并左侧补空格。
fn center_line(line: Line<'static>, width: usize) -> Line<'static> {
    let w: usize = line
        .spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let pad = width.saturating_sub(w) / 2;
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(line.spans);
    Line::from(spans)
}

/// 入口：把 banner 按 ANSI 逐行打到 stdout。
pub fn print_to_stdout(theme: &Theme) {
    use crossterm::style::{ResetColor, SetForegroundColor};
    use std::io::{self, Write};
    let out = io::stdout();
    let mut out = out.lock();
    for line in render(theme) {
        for span in line.spans {
            if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                let _ = crossterm::queue!(
                    out,
                    SetForegroundColor(crossterm::style::Color::Rgb { r, g, b })
                );
            }
            let _ = out.write_all(span.content.as_bytes());
            let _ = crossterm::queue!(out, ResetColor);
        }
        let _ = out.write_all(b"\n");
    }
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_non_empty() {
        let t = Theme::catppuccin_mocha();
        let lines = render(&t);
        assert!(lines.len() > 20, "banner 至少 20 行（含八卦 20 行 + 装饰）");
    }

    #[test]
    fn bagua_is_20_rows() {
        assert_eq!(BAGUA.len(), 20, "八卦字符阵应 20 行");
    }

    #[test]
    fn gradient_bar_fills_width() {
        let ln = gradient_bar("━", 20, Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255));
        // 每个 `━` 1 col，每 char 一个 span
        assert_eq!(ln.spans.len(), 20);
    }

    #[test]
    fn title_includes_all_pieces() {
        let t = Theme::catppuccin_mocha();
        let whole: String = render(&t)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect::<Vec<_>>()
            .join("");
        for s in ["伏", "羲", "F U X I", "v0.1.0", "玄女调度"] {
            assert!(whole.contains(s), "banner 应包含 {s}");
        }
    }
}
