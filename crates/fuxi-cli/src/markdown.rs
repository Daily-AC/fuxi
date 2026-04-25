//! Commonmark markdown → Ratatui `Vec<Line>` 轻量渲染器。
//!
//! 专为 assistant 流式输出用——远端门客（和本地 cc）的文本经常带 markdown 语法
//! （代码块、列表、加粗）。此前全部按纯文本渲染，读代码块和 `**bold**` 眼瞎。
//!
//! ## 支持范围
//!
//! - 段落 (paragraph) —— 段间空行分隔
//! - 加粗 `**bold**` → `Modifier::BOLD`
//! - 斜体 `*em*` / `_em_` → `Modifier::ITALIC`
//! - 行内 code `` `code` `` → 代码配色 + 背景
//! - 围栏代码块 ` ```lang ... ``` ` → 每行独立背景 + code 色
//! - 标题 `#`/`##` → 下一段 BOLD，首行前缀 `# ` / `## `（markdown 味儿保留）
//! - 无序列表 `- item` → `• item`
//! - 有序列表 `1. item` → `1. item`（计数由 pulldown-cmark 返回）
//! - 引用 `> quote` → `┃ ` 前缀（与 rail 呼应——opencode 也这么做）
//!
//! ## 不支持（对 agent 文本常见程度低）
//!
//! - 表格（pulldown-cmark gfm table feature，我们没开）
//! - 图片 / 链接（TUI 里无法 clickthrough，渲染成文本意义不大；保留文字不带链接）
//! - HTML passthrough
//! - 嵌套 block（list 里的 code block 等——会 fallback 成普通文本）
//!
//! 流式友好：parse 一次整段，产出 Lines。调用方每次 agent text 更新时重算
//! （便宜，pulldown-cmark 解析几 KB 级文本是微秒量级）。

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// 解析 markdown 源串成 ratatui `Line` 序列。
///
/// - `base_style` 是正文基础 Style（调用方的 body_style；md 渲染在其上叠加
///   Bold / Italic modifier，颜色通常保留 base）
/// - `th` 提供 code / muted / quote 等 semantic 色
pub(crate) fn md_to_lines(src: &str, base_style: Style, th: &Theme) -> Vec<Line<'static>> {
    let mut r = Renderer::new(base_style, th);
    let opts = Options::ENABLE_STRIKETHROUGH;
    for ev in Parser::new_ext(src, opts) {
        r.handle(ev);
    }
    r.finish()
}

struct Renderer<'t> {
    base: Style,
    th: &'t Theme,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    bold_depth: u32,
    italic_depth: u32,
    strike_depth: u32,
    in_code_block: bool,
    /// `> quote` 激活时每新行前缀 `┃ `。
    quote_depth: u32,
    /// 从最外 list 到当前的嵌套；数字列表在 Start(Tag::Item) 时 post-inc 自增。
    list_stack: Vec<ListState>,
    /// heading 级别（1..=6）— 非零时当前段应 Bold，且首 line flush 时前缀 `# ` 之类。
    heading_level: u8,
    heading_prefix_pushed: bool,
}

#[derive(Debug, Clone, Copy)]
struct ListState {
    /// Some(n) = 有序，n 是下一项序号；None = 无序（bullet）
    next_ordinal: Option<u64>,
}

impl<'t> Renderer<'t> {
    fn new(base: Style, th: &'t Theme) -> Self {
        Self {
            base,
            th,
            lines: Vec::new(),
            current: Vec::new(),
            bold_depth: 0,
            italic_depth: 0,
            strike_depth: 0,
            in_code_block: false,
            quote_depth: 0,
            list_stack: Vec::new(),
            heading_level: 0,
            heading_prefix_pushed: false,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        // 段落/块结束都调 blank_line 分隔——末段后会留一个尾部空行，收尾 pop 掉。
        // 保留段间的空行（只 pop 连续尾部的）。
        while self
            .lines
            .last()
            .is_some_and(|l| l.spans.iter().all(|s| s.content.is_empty()))
        {
            self.lines.pop();
        }
        self.lines
    }

    /// 根据当前 style 叠加器状态生成 Span 用的 Style。
    fn text_style(&self) -> Style {
        let mut s = self.base;
        if self.bold_depth > 0 || self.heading_level > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic_depth > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike_depth > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        s
    }

    fn code_style(&self) -> Style {
        // inline code 用柔和背景 + peach 强调色；与 assistant body 区分开。
        Style::default().fg(self.th.peach).bg(self.th.surface0)
    }

    fn code_block_style(&self) -> Style {
        // block 版用更暗背景 + teal 冷调字——视觉跟 assistant 主文本拉开更大。
        Style::default().fg(self.th.teal).bg(self.th.mantle)
    }

    fn muted(&self) -> Color {
        self.th.muted()
    }

    /// 把 `current` 作为一条 Line 推出——若空则推一个空行。flush 后清 current。
    fn flush_line(&mut self) {
        if self.current.is_empty() {
            // 不推空行除非调用方显式 hard_break（段落结束走 blank_line 另一路径）
            return;
        }
        let spans = std::mem::take(&mut self.current);
        self.lines.push(Line::from(spans));
    }

    /// 段落 / 标题结束时推一个空行分隔。避免重复空行。
    fn blank_line(&mut self) {
        if self.lines.last().is_some_and(|l| !l.spans.is_empty()) {
            self.lines.push(Line::from(""));
        }
    }

    /// 新起一行时若在 blockquote 内部，前缀 `┃ `——跟 rail 视觉呼应。
    fn start_line_prefix(&mut self) {
        for _ in 0..self.quote_depth {
            self.current.push(Span::styled(
                "┃ ".to_string(),
                Style::default().fg(self.muted()),
            ));
        }
    }

    fn push_heading_prefix(&mut self, level: u8) {
        // 保留 markdown 味儿：`# ` / `## ` —— TUI 里 Ratatui 没 heading 原生类型，
        // bold + 前缀双保险。muted 灰的井号让它不抢眼，body 的加粗才是主角。
        let hashes = "#".repeat(level.min(6) as usize);
        self.current.push(Span::styled(
            format!("{hashes} "),
            Style::default()
                .fg(self.muted())
                .add_modifier(Modifier::BOLD),
        ));
    }

    fn push_list_bullet(&mut self) {
        if let Some(state) = self.list_stack.last_mut() {
            let bullet = match state.next_ordinal.as_mut() {
                Some(n) => {
                    let b = format!("{n}. ");
                    *n += 1;
                    b
                }
                None => "• ".to_string(),
            };
            // 嵌套缩进：最外层无前导空格，内层每层 2 空格
            let indent = (self.list_stack.len().saturating_sub(1)) * 2;
            if indent > 0 {
                self.current.push(Span::raw(" ".repeat(indent)));
            }
            self.current
                .push(Span::styled(bullet, Style::default().fg(self.muted())));
        }
    }

    fn handle(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(t) => self.handle_text(&t),
            Event::Code(t) => {
                self.current
                    .push(Span::styled(t.to_string(), self.code_style()));
            }
            Event::SoftBreak => {
                // 源码里单个换行——按 markdown 语义通常当空格（流式文本的显式换行走
                // HardBreak 或空行）。但对中文 / CJK 语境，soft break 在视觉上期望保留
                // 换行。折中：塞单空格，让同段连成流。
                if !self.in_code_block {
                    self.current.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                self.flush_line();
                self.start_line_prefix();
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                // HTML passthrough：当纯文本塞进去——agent 不该吐 html，防御性处理。
            }
            Event::Rule => {
                self.flush_line();
                // `---` 分隔线渲染成一行 `────` （用 unicode box-drawing 而非 ASCII 更干净）
                self.current.push(Span::styled(
                    "────────".to_string(),
                    Style::default().fg(self.muted()),
                ));
                self.flush_line();
            }
            Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {
                // 这些边缘 feature 暂不支持
            }
        }
    }

    fn handle_text(&mut self, t: &str) {
        if self.in_code_block {
            // code block 可能跨多行——按 \n 切，每行独立作为一条 Line
            let mut first_piece = true;
            for line in t.split('\n') {
                if !first_piece {
                    self.flush_line();
                    self.start_line_prefix();
                }
                first_piece = false;
                if !line.is_empty() {
                    self.current
                        .push(Span::styled(line.to_string(), self.code_block_style()));
                }
            }
        } else {
            // heading 第一次文本到来时前缀尚未挂（在 Start(Heading) 里挂 prefix 会在
            // list / quote 前面乱序）——简化：不在 heading 段落里挂前缀了，靠 Bold 做
            // 强调，前缀省掉。否则前缀冒在 list bullet 前面次序不对。
            let _ = self.heading_prefix_pushed;
            self.current
                .push(Span::styled(t.to_string(), self.text_style()));
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.start_line_prefix();
            }
            Tag::Heading { level, .. } => {
                self.heading_level = level as u8;
                self.flush_line();
                self.start_line_prefix();
                self.push_heading_prefix(level as u8);
                self.heading_prefix_pushed = true;
            }
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
                self.flush_line();
                // 进入 quote 块后每行前缀由 start_line_prefix 维护
            }
            Tag::CodeBlock(_) => {
                self.in_code_block = true;
                self.flush_line();
                self.start_line_prefix();
            }
            Tag::List(first_ordinal) => {
                self.flush_line();
                self.list_stack.push(ListState {
                    next_ordinal: first_ordinal,
                });
            }
            Tag::Item => {
                // Item 内部可能嵌多 paragraph，但常见 agent 输出是单行 item，
                // 简化：每个 Item 开启新行 + bullet。
                if !self.current.is_empty() {
                    self.flush_line();
                }
                self.start_line_prefix();
                self.push_list_bullet();
            }
            Tag::Emphasis => self.italic_depth += 1,
            Tag::Strong => self.bold_depth += 1,
            Tag::Strikethrough => self.strike_depth += 1,
            Tag::Link { .. } | Tag::Image { .. } => {
                // 不打开链接——文本 part 会继续通过 Text/Code 事件流出，保留原文
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line();
                self.blank_line();
            }
            TagEnd::Heading(_) => {
                self.heading_level = 0;
                self.heading_prefix_pushed = false;
                self.flush_line();
                self.blank_line();
            }
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.flush_line();
                self.blank_line();
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.flush_line();
                self.blank_line();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.flush_line();
                self.blank_line();
            }
            TagEnd::Item => {
                self.flush_line();
            }
            TagEnd::Emphasis => self.italic_depth = self.italic_depth.saturating_sub(1),
            TagEnd::Strong => self.bold_depth = self.bold_depth.saturating_sub(1),
            TagEnd::Strikethrough => {
                self.strike_depth = self.strike_depth.saturating_sub(1);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(src: &str) -> Vec<String> {
        let lines = md_to_lines(src, Style::default(), &Theme::default());
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn plain_paragraph_is_one_line() {
        let out = render("hello world");
        assert_eq!(out, vec!["hello world".to_string()]);
    }

    #[test]
    fn two_paragraphs_get_blank_between() {
        let out = render("para one\n\npara two");
        assert_eq!(
            out,
            vec![
                "para one".to_string(),
                "".to_string(),
                "para two".to_string()
            ]
        );
    }

    #[test]
    fn bold_italic_and_inline_code_preserve_text() {
        let out = render("**bold** and *em* and `inline`");
        // 所有文字 part 都应出现在最终合并文本里
        let joined = out.join("");
        assert!(joined.contains("bold"), "got: {joined}");
        assert!(joined.contains("em"), "got: {joined}");
        assert!(joined.contains("inline"), "got: {joined}");
    }

    #[test]
    fn bold_applies_modifier() {
        let lines = md_to_lines(
            "normal **loud** normal",
            Style::default(),
            &Theme::default(),
        );
        // 找到 text == "loud" 的 Span 验证 BOLD
        let loud = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "loud")
            .expect("应能找到 'loud' span");
        assert!(
            loud.style.add_modifier.contains(Modifier::BOLD),
            "loud span 应加粗"
        );
    }

    #[test]
    fn code_block_preserves_lines_and_styles() {
        let src = "```\nline1\nline2\n```";
        let lines = md_to_lines(src, Style::default(), &Theme::default());
        let bodies: Vec<&str> = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(bodies.contains(&"line1"), "got: {bodies:?}");
        assert!(bodies.contains(&"line2"), "got: {bodies:?}");
        // code block span 应带背景色（非 Color::Reset）
        let has_bg = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| matches!(s.content.as_ref(), "line1" | "line2") && s.style.bg.is_some());
        assert!(has_bg, "code block 应有背景色");
    }

    #[test]
    fn unordered_list_uses_bullet() {
        let out = render("- a\n- b");
        let joined = out.join("\n");
        assert!(joined.contains("• a"), "got:\n{joined}");
        assert!(joined.contains("• b"), "got:\n{joined}");
    }

    #[test]
    fn ordered_list_uses_numbers() {
        let out = render("1. first\n2. second");
        let joined = out.join("\n");
        assert!(joined.contains("1. first"), "got:\n{joined}");
        assert!(joined.contains("2. second"), "got:\n{joined}");
    }

    #[test]
    fn blockquote_gets_rail_prefix() {
        let out = render("> 引用一段");
        let joined = out.join("\n");
        assert!(
            joined.contains("┃ 引用一段"),
            "blockquote 应以 ┃ 开头，got:\n{joined}"
        );
    }

    #[test]
    fn heading_applies_bold_modifier_to_body() {
        let lines = md_to_lines("# 标题内容", Style::default(), &Theme::default());
        let body = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.as_ref() == "标题内容")
            .expect("应找到 heading body");
        assert!(
            body.style.add_modifier.contains(Modifier::BOLD),
            "heading body 应 bold"
        );
    }

    /// 空字符串不应炸。
    #[test]
    fn empty_input_yields_no_lines() {
        let lines = md_to_lines("", Style::default(), &Theme::default());
        assert!(lines.is_empty());
    }

    /// 单字符输入应产出一条非空 Line——回归保护：极小输入不该被段落收尾的
    /// trailing-blank 清理误伤。
    #[test]
    fn single_char_input() {
        let lines = md_to_lines("a", Style::default(), &Theme::default());
        assert_eq!(lines.len(), 1, "单字符应产出一条 Line");
        let body: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(body, "a");
    }

    /// 单段长行：渲染器本身不做换行（折行交给 ratatui 上层），但**必须**把 500
    /// 字符完整保留在一条 Line 内，不丢字、不 panic、不爆栈。
    #[test]
    fn very_long_line_wraps() {
        let src: String = "x".repeat(500);
        let lines = md_to_lines(&src, Style::default(), &Theme::default());
        assert_eq!(lines.len(), 1);
        let body: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(body.len(), 500, "500 字符应原样保留");
        assert!(body.chars().all(|c| c == 'x'));
    }

    /// blockquote 内嵌列表：每个 item 行应同时带 `┃ ` 前缀和 bullet。
    #[test]
    fn nested_list_in_blockquote() {
        let out = render("> - item one\n> - item two");
        let joined = out.join("\n");
        assert!(
            joined.contains("┃ ") && joined.contains("• item one"),
            "blockquote 内的 list item 应同时出现 rail 前缀和 bullet，got:\n{joined}"
        );
        assert!(
            joined.contains("• item two"),
            "第二个 item 应渲染，got:\n{joined}"
        );
    }

    /// 代码块内单行 200 字符——回归保护 markdown.rs:244-247 的 long-line
    /// 路径：单行内容应进入一个 Span，长度精确保留。
    #[test]
    fn code_block_with_long_line() {
        let long: String = "y".repeat(200);
        let src = format!("```\n{long}\n```");
        let lines = md_to_lines(&src, Style::default(), &Theme::default());
        let body_line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.len() == 200))
            .expect("应存在长度 200 的 code span");
        let body: String = body_line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(body.len(), 200);
        assert!(body.chars().all(|c| c == 'y'));
    }

    /// CJK 含中文标点的长行：确保 String 切片以 char 为单位，不会破中文 UTF-8
    /// 字节边界。渲染器整段塞进 Span，文本应字节级完全等同输入。
    #[test]
    fn cjk_line_break() {
        // 重复中文片段到一个较长 paragraph，包含中文标点（，。「」）
        let chunk = "你好，世界。「测试」";
        let src: String = chunk.repeat(20);
        let lines = md_to_lines(&src, Style::default(), &Theme::default());
        assert_eq!(lines.len(), 1);
        let body: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        // 字节数完全相同 = 没有破字符
        assert_eq!(body.as_bytes(), src.as_bytes());
        // 中文标点出现次数应保留
        assert_eq!(body.matches('，').count(), 20);
        assert_eq!(body.matches('「').count(), 20);
    }

    /// 空围栏代码块：` ``` \n``` ` 不应 panic，应输出至多一个空段（pop trailing
    /// blank 之后允许为空）。
    #[test]
    fn empty_code_block() {
        let lines = md_to_lines("```\n```", Style::default(), &Theme::default());
        // 不应 panic；可允许 0 行（finish 把尾部空行 pop 干净）
        for l in &lines {
            for s in &l.spans {
                let _ = s.content.as_ref();
            }
        }
        assert!(lines.len() <= 1, "空 code block 不该堆出多行内容");
    }
}
