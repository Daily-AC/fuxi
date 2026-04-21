//! 斜杠命令自动补全浮层——纯状态机，宿主（repl）只需喂键、拿 `PopupEvent` 处理。
//
// 设计背景：
// - R8 要的不是 UI，而是一个**不依赖 tui 上下文**的纯状态机：
//   `SlashPopup::handle_key` 吞 crossterm 的 (KeyCode, KeyModifiers)，返回
//   `PopupEvent` 告诉宿主"关闭"/"执行某命令"/"什么都别做"。
//   这样：
//   1. 不碰 repl.rs——整合工作交给下游（γ 第二波 / β）。
//   2. 单测好写，不需要起 ratatui backend。
//   3. 宿主可以随时把 popup 模块替掉换成别的补全 UI，不影响 CommandRegistry。
// - `candidates` 存 `Command` 的 **快照 Vec**（clone）而不是 `&Command` 引用：
//   Rust 生命周期 + 可变状态机不好接，状态机不能持有外部 registry 的借用。
//   Command 只含 `&'static str` + 枚举，clone 成本可忽略。
// - `PopupEvent::Execute(CommandAction)` 直接返 action 本体而非命令名：
//   宿主拿到就能 match，一步到位；不用再回查 registry。
// - Fuzzy 策略 delegate 到 `CommandRegistry::fuzzy_filter`——R7 已定前缀不分大小写。
//   状态机只负责"键入 → 过滤 → 选中"的编排，不重复实现匹配逻辑。
// - Backspace 删到空且之前是空时关闭浮层——模拟 VSCode/Helix 的自然退出：
//   用户打了个 `/` 反悔了，一路 Backspace 回去就该消失。
//
// WHY `allow(dead_code)`：宿主整合（repl 里触发 `open` / 绑 key routing / 渲染
// overlay）是 γ 第二波的活。当下只交数据层 + 状态机，移除 allow 的时机 = 宿主 use 起来后。

#![allow(dead_code)]

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::command_registry::{Command, CommandAction, CommandRegistry};
use crate::theme::Theme;

/// 状态机把事件消化后，要求宿主做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupEvent {
    /// 按键被吞了但没产生副作用（纯内部过滤/导航）。
    None,
    /// 请宿主把 popup 关掉（Esc、Backspace 一路删空、无候选时 Enter 等）。
    Close,
    /// 请宿主执行选中命令的 action，并同时把 popup 关掉。
    Execute(CommandAction),
}

/// 斜杠命令浮层。
///
/// **不持有 `CommandRegistry` 引用**——每次 `open` / `handle_key` 都要外部传入。
/// 原因见模块注释（避免与宿主状态的生命周期绑死）。
#[derive(Debug, Clone, Default)]
pub struct SlashPopup {
    open: bool,
    /// 用户已键入的过滤前缀，**不含** 引导 `/`——展示用 `display_input()` 再补上。
    filter: String,
    /// 当前高亮选中项在 `candidates` 里的下标。
    selected: usize,
    /// 过滤后候选的快照。`open` / 过滤变动时重建。
    candidates: Vec<Command>,
}

impl SlashPopup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 打开浮层——初始过滤为空（全部命令都显示）。
    pub fn open(&mut self, registry: &CommandRegistry) {
        self.open = true;
        self.filter.clear();
        self.selected = 0;
        self.refresh_candidates(registry);
    }

    /// 关闭并清状态——下次 `open` 回到干净起点。
    pub fn close(&mut self) {
        self.open = false;
        self.filter.clear();
        self.selected = 0;
        self.candidates.clear();
    }

    /// 当前输入框展示串：`"/" + filter`。宿主要在浮层底部或输入区展示这个。
    pub fn display_input(&self) -> String {
        let mut s = String::with_capacity(self.filter.len() + 1);
        s.push('/');
        s.push_str(&self.filter);
        s
    }

    /// 当前候选快照（宿主 render 用）。外部不该改，所以只给 slice。
    pub fn candidates(&self) -> &[Command] {
        &self.candidates
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// 键事件处理——状态机核心。
    ///
    /// 约定：
    /// - `Esc` → `Close`
    /// - `Up` / `Ctrl+P` → 向上导航（在 [0, len) 回环）
    /// - `Down` / `Ctrl+N` → 向下导航
    /// - `Enter` → 选中时 `Execute(action)`，无候选时 `Close`
    /// - 可打印字符（`KeyCode::Char(c)` 且 `c != '/'`）→ 追加到 filter，重刷
    /// - `Backspace` → filter 非空则删末字符重刷；filter 已空 → `Close`
    /// - 其他按键 → `None`（宿主自行决定是否吃掉）
    pub fn handle_key(
        &mut self,
        key: KeyCode,
        mods: KeyModifiers,
        registry: &CommandRegistry,
    ) -> PopupEvent {
        if !self.open {
            return PopupEvent::None;
        }

        match key {
            KeyCode::Esc => {
                self.close();
                PopupEvent::Close
            }
            KeyCode::Up => {
                self.move_up();
                PopupEvent::None
            }
            KeyCode::Down => {
                self.move_down();
                PopupEvent::None
            }
            // Ctrl+P / Ctrl+N 是终端惯用上下移——许多用户从 zsh/emacs 迁移过来默认这套。
            KeyCode::Char('p') if mods.contains(KeyModifiers::CONTROL) => {
                self.move_up();
                PopupEvent::None
            }
            KeyCode::Char('n') if mods.contains(KeyModifiers::CONTROL) => {
                self.move_down();
                PopupEvent::None
            }
            KeyCode::Enter => {
                if let Some(cmd) = self.candidates.get(self.selected) {
                    let action = cmd.action.clone();
                    self.close();
                    PopupEvent::Execute(action)
                } else {
                    // 没候选按 Enter = 放弃——闭合浮层让宿主把原文当普通输入处理。
                    self.close();
                    PopupEvent::Close
                }
            }
            KeyCode::Backspace => {
                if self.filter.is_empty() {
                    self.close();
                    PopupEvent::Close
                } else {
                    self.filter.pop();
                    self.refresh_candidates(registry);
                    PopupEvent::None
                }
            }
            KeyCode::Char(c) => {
                // '/' 是引导符——不允许出现在 filter 里，静默吃掉。
                if c == '/' {
                    return PopupEvent::None;
                }
                // Ctrl+其他字符 = 快捷键，不当可打印字符处理。
                if mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::ALT) {
                    return PopupEvent::None;
                }
                self.filter.push(c);
                self.refresh_candidates(registry);
                PopupEvent::None
            }
            _ => PopupEvent::None,
        }
    }

    /// 生成浮层内容行——给宿主绝对定位渲染用。
    ///
    /// 布局（每条候选一行，若干行）：
    /// ```text
    /// ▍ /help   列出全部命令
    ///   /clear  清空滚屏
    /// ```
    /// 选中行左边留一根竖条 `▍`，其余行留同宽的空白占位保证列对齐。
    pub fn render_lines<'a>(&'a self, theme: &Theme) -> Vec<Line<'a>> {
        if !self.open || self.candidates.is_empty() {
            return Vec::new();
        }

        // 让所有行的 slash 列宽度对齐——取最长 slash 的 char 数。
        let slash_width = self
            .candidates
            .iter()
            .map(|c| c.slash.chars().count())
            .max()
            .unwrap_or(0);

        let selected_style = Style::default()
            .fg(theme.focus_border())
            .add_modifier(Modifier::BOLD);
        let normal_style = Style::default().fg(theme.text);
        let desc_style = Style::default().fg(theme.muted());

        self.candidates
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                let is_sel = i == self.selected;
                let marker = if is_sel { "▍ " } else { "  " };
                let row_style = if is_sel { selected_style } else { normal_style };
                let padded_slash = pad_right(cmd.slash, slash_width);
                Line::from(vec![
                    Span::styled(marker, row_style),
                    Span::styled(padded_slash, row_style),
                    Span::raw("  "),
                    Span::styled(cmd.description, desc_style),
                ])
            })
            .collect()
    }

    // —— 内部 ——

    fn refresh_candidates(&mut self, registry: &CommandRegistry) {
        // CommandRegistry::fuzzy_filter 接受 "/help" 前缀；这里把内部 filter 补 '/'。
        let mut needle = String::with_capacity(self.filter.len() + 1);
        needle.push('/');
        needle.push_str(&self.filter);
        self.candidates = registry
            .fuzzy_filter(&needle)
            .into_iter()
            .cloned()
            .collect();
        // selected 越界时夹回 0——候选列表收缩后防 OOB。
        if self.selected >= self.candidates.len() {
            self.selected = 0;
        }
    }

    fn move_up(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.candidates.len() - 1
        } else {
            self.selected - 1
        };
    }

    fn move_down(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.candidates.len();
    }
}

/// 按 char count 右填充空格——unicode 宽字符场景这里不做严格 column 对齐，
/// 只保证「行都一样多 char」。命令名是 ASCII `/abc` 所以 char count == column。
fn pad_right(s: &str, target_chars: usize) -> String {
    let cur = s.chars().count();
    if cur >= target_chars {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (target_chars - cur));
    out.push_str(s);
    for _ in 0..(target_chars - cur) {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_registry::register_default;

    fn reg() -> CommandRegistry {
        register_default()
    }

    // —— 契约六件套（team-lead 指定）——

    #[test]
    fn popup_opens_with_filter_empty() {
        let r = reg();
        let mut p = SlashPopup::new();
        assert!(!p.is_open());

        p.open(&r);
        assert!(p.is_open(), "open() 后必须 is_open()");
        assert_eq!(p.display_input(), "/", "初始展示只有 '/' 引导符");
        // 初始候选 = 全量（fuzzy_filter("/") 命中全部）。
        assert_eq!(p.candidates().len(), r.all().len());
        assert_eq!(p.selected_index(), 0);
    }

    #[test]
    fn typing_filters_candidates_with_fuzzy() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);

        // 键入 'h' → 过滤前缀变成 "/h" → 只剩 /help。
        let ev = p.handle_key(KeyCode::Char('h'), KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::None);
        assert_eq!(p.display_input(), "/h");
        assert_eq!(p.candidates().len(), 1);
        assert_eq!(p.candidates()[0].slash, "/help");

        // 键入 '/' 应被静默吞掉（filter 不加 '/'）。
        let ev = p.handle_key(KeyCode::Char('/'), KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::None);
        assert_eq!(p.display_input(), "/h");
    }

    #[test]
    fn arrow_keys_navigate_selection() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        let n = p.candidates().len();
        assert!(n >= 2, "默认 registry 至少两条命令才能测导航");

        // 下一次。
        assert_eq!(
            p.handle_key(KeyCode::Down, KeyModifiers::NONE, &r),
            PopupEvent::None
        );
        assert_eq!(p.selected_index(), 1);

        // 上一次回到 0。
        assert_eq!(
            p.handle_key(KeyCode::Up, KeyModifiers::NONE, &r),
            PopupEvent::None
        );
        assert_eq!(p.selected_index(), 0);

        // 从 0 往上 = 回环到末尾。
        assert_eq!(
            p.handle_key(KeyCode::Up, KeyModifiers::NONE, &r),
            PopupEvent::None
        );
        assert_eq!(p.selected_index(), n - 1);

        // 从末尾往下 = 回环到 0。
        assert_eq!(
            p.handle_key(KeyCode::Down, KeyModifiers::NONE, &r),
            PopupEvent::None
        );
        assert_eq!(p.selected_index(), 0);

        // Ctrl+N / Ctrl+P 等价 Down/Up。
        p.handle_key(KeyCode::Char('n'), KeyModifiers::CONTROL, &r);
        assert_eq!(p.selected_index(), 1);
        p.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL, &r);
        assert_eq!(p.selected_index(), 0);
    }

    #[test]
    fn enter_returns_execute_event_with_action() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);

        // 第 0 项是 /help——默认 registry 定义。
        let ev = p.handle_key(KeyCode::Enter, KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::Execute(CommandAction::Help));
        // Enter 后 popup 自闭合。
        assert!(!p.is_open());
    }

    #[test]
    fn esc_returns_close_event() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);

        let ev = p.handle_key(KeyCode::Esc, KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::Close);
        assert!(!p.is_open());
    }

    #[test]
    fn backspace_at_empty_filter_returns_close() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        // filter 本就是空——一次 Backspace 应关闭。
        let ev = p.handle_key(KeyCode::Backspace, KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::Close);
        assert!(!p.is_open());
    }

    // —— 不变量补测 ——

    #[test]
    fn backspace_with_filter_shrinks_not_closes() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        p.handle_key(KeyCode::Char('h'), KeyModifiers::NONE, &r);
        p.handle_key(KeyCode::Char('e'), KeyModifiers::NONE, &r);
        assert_eq!(p.display_input(), "/he");

        let ev = p.handle_key(KeyCode::Backspace, KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::None);
        assert!(p.is_open());
        assert_eq!(p.display_input(), "/h");
    }

    #[test]
    fn typing_shrinks_selected_back_to_zero() {
        // selected 停在非 0，过滤后只剩 1 条——selected 应夹回 0，不能越界。
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        // 先挪到末尾。
        p.handle_key(KeyCode::Up, KeyModifiers::NONE, &r);
        let n_before = p.candidates().len();
        assert_eq!(p.selected_index(), n_before - 1);

        // 键入 'h' 后 candidates 只剩 1 条，selected 该被夹到 0 避免 OOB。
        p.handle_key(KeyCode::Char('h'), KeyModifiers::NONE, &r);
        assert_eq!(p.candidates().len(), 1);
        assert_eq!(p.selected_index(), 0);

        // 此时 Enter 应出 Help action，不能 panic。
        let ev = p.handle_key(KeyCode::Enter, KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::Execute(CommandAction::Help));
    }

    #[test]
    fn enter_with_no_candidates_closes() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        // 造一个绝不会匹配的 filter——键入几个不存在的字符。
        for c in ['z', 'z', 'z'] {
            p.handle_key(KeyCode::Char(c), KeyModifiers::NONE, &r);
        }
        assert!(p.candidates().is_empty());

        let ev = p.handle_key(KeyCode::Enter, KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::Close);
        assert!(!p.is_open());
    }

    #[test]
    fn handle_key_while_closed_is_noop() {
        let r = reg();
        let mut p = SlashPopup::new();
        // 没 open 就喂键——应返 None 且不崩。
        let ev = p.handle_key(KeyCode::Char('h'), KeyModifiers::NONE, &r);
        assert_eq!(ev, PopupEvent::None);
        assert!(!p.is_open());
    }

    #[test]
    fn ctrl_alt_modifier_char_is_ignored() {
        // Ctrl+其他字母 / Alt+字母 不进 filter——避免 Ctrl+S 之类误触。
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        p.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL, &r);
        assert_eq!(p.display_input(), "/");
        p.handle_key(KeyCode::Char('a'), KeyModifiers::ALT, &r);
        assert_eq!(p.display_input(), "/");
    }

    #[test]
    fn close_clears_state() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        p.handle_key(KeyCode::Char('h'), KeyModifiers::NONE, &r);
        assert!(p.is_open());
        assert_eq!(p.display_input(), "/h");

        p.close();
        assert!(!p.is_open());
        assert_eq!(p.display_input(), "/");
        assert!(p.candidates().is_empty());
    }

    #[test]
    fn render_lines_empty_when_closed() {
        let p = SlashPopup::new();
        let theme = Theme::default();
        assert!(p.render_lines(&theme).is_empty());
    }

    #[test]
    fn render_lines_has_one_line_per_candidate() {
        let r = reg();
        let mut p = SlashPopup::new();
        p.open(&r);
        let theme = Theme::default();
        let lines = p.render_lines(&theme);
        assert_eq!(lines.len(), p.candidates().len());
    }
}
