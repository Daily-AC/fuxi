//! Firehose TUI —— 把 `Stream<Event>` 渲染成一个活的仪表盘。
//!
//! 结构：
//! - `FirehoseApp`：纯状态 + 渲染器。不自己 spawn 任务，也不 own 流——上层（CLI）
//!   驱动事件循环，把接收到的 `Event` 喂进 `ingest`；按键走 `handle_key`；
//!   每一帧调 `draw(frame)`。
//! - `EventRow`：每条事件的缓存展示行，避免每帧重复格式化。
//!
//! 设计取舍：
//! - 为什么不自带 runtime：ratatui + crossterm 的 terminal lifecycle 属于二进制层的
//!   职责，library 层不碰它——便于测试用 `TestBackend` 注入 + CLI 可以并行跑多视图。
//! - 为什么用环形缓冲：事件速率可能高，内存有界；`VecDeque::len()` 上限 10_000。

use fuxi_core::{Event, EventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// 允许的最大历史——超过后丢最旧的。
pub const MAX_BUFFER: usize = 10_000;

/// 用来算“事件/秒”的滚动窗口大小。
pub const RATE_WINDOW: Duration = Duration::from_secs(5);

/// 一条显示行的预格式化快照。
#[derive(Debug, Clone)]
pub struct EventRow {
    /// HH:MM:SS.
    pub time: String,
    /// agent 简短标识——没有就填 `platform`。
    pub who: String,
    /// 事件类型 tag（同 EventStore 里的 kind_tag）。
    pub kind_tag: &'static str,
    /// 单行摘要。
    pub summary: String,
    /// 渲染颜色——按 kind 大类分配。
    pub color: Color,
    /// 原事件里的 ingest 时刻（本地时钟）——用来算 rate。
    pub ingested_at: Instant,
}

impl EventRow {
    /// 从 `Event` 构造一行。
    pub fn from_event(ev: &Event) -> Self {
        let time = ev.meta.at.format("%H:%M:%S").to_string();
        let who = ev
            .meta
            .agent
            .map(|a| short_id(&a.to_string()))
            .unwrap_or_else(|| "platform".to_string());
        let kind_tag = kind_tag(&ev.kind);
        let summary = summarize(&ev.kind);
        let color = color_for(&ev.kind);
        Self {
            time,
            who,
            kind_tag,
            summary,
            color,
            ingested_at: Instant::now(),
        }
    }
}

/// 可复用的 Firehose TUI 应用。
#[derive(Debug)]
pub struct FirehoseApp {
    rows: VecDeque<EventRow>,
    agents_seen: HashSet<String>,
    /// 当前是否“粘底”自动滚——用户向上翻的瞬间解除。
    auto_scroll: bool,
    /// 列表选中行 / 视图偏移。
    list_state: ListState,
    /// 过滤器：大小写不敏感的子串，匹配 `kind_tag`。
    filter: Option<String>,
    /// 过滤模式缓冲区。
    filter_input: Option<String>,
    /// 若 true 则 `handle_key(q)` 会把它设 true；上层观察此标志退出循环。
    should_quit: bool,
}

impl Default for FirehoseApp {
    fn default() -> Self {
        let mut list_state = ListState::default();
        list_state.select(None);
        Self {
            rows: VecDeque::with_capacity(1024),
            agents_seen: HashSet::new(),
            auto_scroll: true,
            list_state,
            filter: None,
            filter_input: None,
            should_quit: false,
        }
    }
}

impl FirehoseApp {
    /// 构造空实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 上层轮询——true 表示应该退出。
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// 处理一次按键。兼容 crossterm 的 `KeyCode` 取值。
    ///
    /// 为什么接 `crossterm::event::KeyCode` 而非自造 enum：避免再造一层映射；
    /// 测试里可以直接构造 `KeyCode::Char('q')`。
    pub fn handle_key(&mut self, key: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;

        // 过滤输入模式下按键走 filter 缓冲。
        if let Some(ref mut buf) = self.filter_input {
            match key {
                KeyCode::Esc => {
                    self.filter_input = None;
                }
                KeyCode::Enter => {
                    let s = std::mem::take(buf);
                    self.filter_input = None;
                    self.filter = if s.is_empty() {
                        None
                    } else {
                        Some(s.to_lowercase())
                    };
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return;
        }

        match key {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('/') => self.filter_input = Some(String::new()),
            KeyCode::Esc => self.filter = None,
            KeyCode::Char('g') => {
                self.auto_scroll = true;
                self.list_state.select(None);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.auto_scroll = false;
                let visible = self.visible_rows();
                let total = visible.len();
                if total == 0 {
                    return;
                }
                let current = self
                    .list_state
                    .selected()
                    .unwrap_or(total.saturating_sub(1));
                self.list_state.select(Some(current.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.auto_scroll = false;
                let visible = self.visible_rows();
                let total = visible.len();
                if total == 0 {
                    return;
                }
                let current = self.list_state.selected().unwrap_or(0);
                let next = (current + 1).min(total - 1);
                self.list_state.select(Some(next));
            }
            _ => {}
        }
    }

    /// 吃进一条事件——更新所有内部状态。
    pub fn ingest(&mut self, ev: &Event) {
        if let Some(a) = ev.meta.agent {
            self.agents_seen.insert(a.to_string());
        }
        let row = EventRow::from_event(ev);
        if self.rows.len() == MAX_BUFFER {
            self.rows.pop_front();
            // 索引相对也随之漂移——但我们选中状态是一个“相对于 visible”的 index，
            // 这里无需补偿；若用户在 buffer 剧烈滚动时看到一瞬跳，接受。
        }
        self.rows.push_back(row);
    }

    /// 渲染一帧。
    pub fn draw(&mut self, f: &mut Frame<'_>) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_top_bar(f, chunks[0]);
        self.draw_list(f, chunks[1]);
        self.draw_bottom_bar(f, chunks[2]);
    }

    fn draw_top_bar(&self, f: &mut Frame<'_>, area: Rect) {
        let now = Instant::now();
        let rate = self
            .rows
            .iter()
            .rev()
            .take_while(|r| now.duration_since(r.ingested_at) <= RATE_WINDOW)
            .count() as f64
            / RATE_WINDOW.as_secs_f64();

        let filter_state = if let Some(buf) = &self.filter_input {
            format!(" filter> {buf}_")
        } else if let Some(f) = &self.filter {
            format!(" filter={f}")
        } else {
            String::new()
        };

        let text = format!(
            " agents={} | events/s={:.1} | buffer={}{}",
            self.agents_seen.len(),
            rate,
            self.rows.len(),
            filter_state
        );
        let para = Paragraph::new(text).style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        f.render_widget(para, area);
    }

    fn draw_list(&mut self, f: &mut Frame<'_>, area: Rect) {
        let visible = self.visible_rows();
        let height = area.height as usize;

        // auto_scroll：确保展示窗口贴底（最新事件在最后一行）。
        let items: Vec<ListItem> = visible
            .iter()
            .map(|r| {
                let line = Line::from(vec![
                    Span::styled(r.time.clone(), Style::default().fg(Color::DarkGray)),
                    Span::raw(" "),
                    Span::styled(
                        format!("{:<12}", short(&r.who, 12)),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("{:<22}", r.kind_tag),
                        Style::default().fg(r.color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::raw(r.summary.clone()),
                ]);
                ListItem::new(line)
            })
            .collect();

        if self.auto_scroll {
            // 把选中状态设到最后一条——Ratatui 的 List 会自动把它带到视窗底部。
            if !visible.is_empty() {
                self.list_state.select(Some(visible.len() - 1));
            } else {
                self.list_state.select(None);
            }
        } else {
            // 非自动滚：保持 list_state.selected 合法。
            if let Some(sel) = self.list_state.selected()
                && sel >= visible.len()
            {
                self.list_state.select(visible.len().checked_sub(1));
            }
        }

        let title = if let Some(f) = &self.filter {
            format!(" events · filter={f} ")
        } else {
            " events ".to_string()
        };

        let list = List::new(items)
            .block(Block::default().borders(Borders::NONE).title(title))
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::REVERSED)
                    .fg(Color::White),
            );

        // ratatui 自己负责让 selected 尽量居中或贴底——为 TestBackend snap 的稳定性
        // 我们只关心最后 N 条的顺序，buffer 里就会是那些内容。
        let _ = height;
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn draw_bottom_bar(&self, f: &mut Frame<'_>, area: Rect) {
        let hint = " q quit | ↑/↓ or k/j scroll | g jump-to-bottom | / filter | Esc clear-filter ";
        let para = Paragraph::new(hint).style(Style::default().fg(Color::Black).bg(Color::Gray));
        f.render_widget(para, area);
    }

    /// 当前可见的行（应用 filter 之后）。
    pub fn visible_rows(&self) -> Vec<&EventRow> {
        match &self.filter {
            None => self.rows.iter().collect(),
            Some(f) => self
                .rows
                .iter()
                .filter(|r| r.kind_tag.to_lowercase().contains(f.as_str()))
                .collect(),
        }
    }

    /// 当前是否处在 filter 输入模式——UI 层可以据此切光标。
    pub fn is_filter_input(&self) -> bool {
        self.filter_input.is_some()
    }

    /// 当前 filter 字符串（小写）——给测试 / 调试看。
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }
}

fn short(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

/// `agent-<uuid>` → 前 8 位简短 id。
fn short_id(s: &str) -> String {
    match s.rsplit_once('-') {
        Some((_, uuid)) => uuid.chars().take(8).collect(),
        None => s.chars().take(8).collect(),
    }
}

/// `EventKind` → 单行摘要。WHY 注释：我们刻意**不是**重复 Debug——人眼更关心“谁说了什么”。
fn summarize(k: &EventKind) -> String {
    use EventKind::*;
    match k {
        AgentSpawning { role, cli } => format!("role={role} cli={cli}"),
        AgentReady { endpoint } => format!("endpoint={endpoint}"),
        AgentShuttingDown { reason } => format!("reason={reason}"),
        AgentDead { cause } => format!("cause={cause}"),
        TaskCreated { title, .. } => format!("title={title}"),
        TaskDispatched { to } => format!("to={to}"),
        TaskStateChanged { from, to } => format!("{from:?} → {to:?}"),
        TaskDelivered { artifact } => format!(
            "artifact={}",
            artifact.to_string().chars().take(60).collect::<String>()
        ),
        TaskBlocked { reason } => format!("reason={reason}"),
        TaskCancelled { reason } => format!("reason={reason}"),
        MessageSent { from, to, text } => format!(
            "{} → {}: {}",
            short_id(&from.to_string()),
            short_id(&to.to_string()),
            one_line(text, 60)
        ),
        MessageReceived { from, text } => format!(
            "from={}: {}",
            short_id(&from.to_string()),
            one_line(text, 60)
        ),
        UserPrompted { text } => format!("user> {}", one_line(text, 60)),
        AgentResponded { text } => format!("agent> {}", one_line(text, 60)),
        ThinkingStarted => "thinking…".to_string(),
        ThinkingFinished => "thinking done".to_string(),
        ToolCallStarted { tool, .. } => format!("tool={tool}"),
        ToolCallFinished {
            tool,
            ok,
            output_preview,
        } => {
            let status = if *ok { "ok" } else { "err" };
            format!("tool={tool} {status}: {}", one_line(output_preview, 40))
        }
        UserInterventionSent { target, mode, text } => {
            format!(
                "→{} [{}]: {}",
                short_id(&target.to_string()),
                mode,
                one_line(text, 40)
            )
        }
        AgentInterrupted { reason } => format!("interrupted: {reason}"),
        TaskInterventionApplied { mode } => format!("intervention applied [{mode}]"),
        OrchestratorCcReceived { from_user_to, text } => format!(
            "cc {} {}",
            short_id(&from_user_to.to_string()),
            one_line(text, 40)
        ),
        ConversationTransferred { from, to, reason } => format!(
            "{}→{} reason={}",
            short_id(&from.to_string()),
            short_id(&to.to_string()),
            reason
        ),
        ConversationReturned { from, to, brief } => format!(
            "{}→{} {}",
            short_id(&from.to_string()),
            short_id(&to.to_string()),
            brief.clone().unwrap_or_default()
        ),
        PlatformStarted { version } => format!("fuxi {version}"),
        PlatformStopping => "platform stopping".to_string(),
        Custom { label, .. } => format!("custom[{label}]"),
    }
}

fn one_line(s: &str, max: usize) -> String {
    let collapsed: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let trimmed: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        format!("{trimmed}…")
    }
}

/// 色盘——大类一个色。为什么不每个 variant 一个颜色：人眼只能分辨有限几档。
fn color_for(k: &EventKind) -> Color {
    use EventKind::*;
    match k {
        AgentSpawning { .. }
        | AgentReady { .. }
        | AgentShuttingDown { .. }
        | AgentDead { .. }
        | PlatformStarted { .. }
        | PlatformStopping => Color::Magenta,

        TaskCreated { .. }
        | TaskDispatched { .. }
        | TaskStateChanged { .. }
        | TaskDelivered { .. }
        | TaskBlocked { .. }
        | TaskCancelled { .. } => Color::Green,

        MessageSent { .. }
        | MessageReceived { .. }
        | UserPrompted { .. }
        | AgentResponded { .. }
        | ThinkingStarted
        | ThinkingFinished => Color::Cyan,

        ToolCallStarted { .. } | ToolCallFinished { .. } => Color::Blue,

        UserInterventionSent { .. }
        | AgentInterrupted { .. }
        | TaskInterventionApplied { .. }
        | OrchestratorCcReceived { .. }
        | ConversationTransferred { .. }
        | ConversationReturned { .. } => Color::Yellow,

        Custom { .. } => Color::DarkGray,
    }
}

/// kind → serde tag；与 events crate 对齐。
pub fn kind_tag(kind: &EventKind) -> &'static str {
    crate::hub::kind_tag(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use fuxi_core::{AgentId, EventMeta};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn mk(label: &str) -> Event {
        Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: label.into(),
                payload: serde_json::json!({}),
            },
        }
    }

    fn mk_with_agent(agent: AgentId, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        Event { meta, kind }
    }

    /// 把 buffer 的某一行渲染成“可读 string”——去掉颜色、保留文本。
    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        let mut s = String::new();
        for x in 0..area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.trim_end().to_string()
    }

    #[test]
    fn ingest_accumulates_rows_and_agents() {
        let mut app = FirehoseApp::new();
        let a = AgentId::new();
        let b = AgentId::new();
        app.ingest(&mk_with_agent(
            a,
            EventKind::Custom {
                label: "x".into(),
                payload: serde_json::json!({}),
            },
        ));
        app.ingest(&mk_with_agent(
            b,
            EventKind::Custom {
                label: "y".into(),
                payload: serde_json::json!({}),
            },
        ));
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.agents_seen.len(), 2);
    }

    #[test]
    fn buffer_cap_is_enforced() {
        let mut app = FirehoseApp::new();
        for i in 0..(MAX_BUFFER + 25) {
            app.ingest(&mk(&format!("e-{i}")));
        }
        assert_eq!(app.rows.len(), MAX_BUFFER);
    }

    #[test]
    fn handle_q_requests_quit() {
        let mut app = FirehoseApp::new();
        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit());
    }

    #[test]
    fn filter_enter_and_escape() {
        let mut app = FirehoseApp::new();
        app.handle_key(KeyCode::Char('/'));
        assert!(app.is_filter_input());
        app.handle_key(KeyCode::Char('t'));
        app.handle_key(KeyCode::Char('A')); // 大小写归一化到小写
        app.handle_key(KeyCode::Char('s'));
        app.handle_key(KeyCode::Char('k'));
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.filter(), Some("task"));
        app.handle_key(KeyCode::Esc);
        assert!(app.filter().is_none());
    }

    #[test]
    fn filter_narrows_visible_rows() {
        let mut app = FirehoseApp::new();
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::TaskCreated {
                title: "t".into(),
                description: "d".into(),
            },
        });
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::PlatformStarted {
                version: "0.1".into(),
            },
        });

        // filter = "task" 只保留 task_*
        app.handle_key(KeyCode::Char('/'));
        for c in "task".chars() {
            app.handle_key(KeyCode::Char(c));
        }
        app.handle_key(KeyCode::Enter);
        let visible = app.visible_rows();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].kind_tag, "task_created");
    }

    #[test]
    fn g_restores_autoscroll() {
        let mut app = FirehoseApp::new();
        app.ingest(&mk("a"));
        app.ingest(&mk("b"));
        app.handle_key(KeyCode::Up);
        assert!(!app.auto_scroll);
        app.handle_key(KeyCode::Char('g'));
        assert!(app.auto_scroll);
    }

    #[test]
    fn snapshot_render_contains_event_rows() {
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = FirehoseApp::new();
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::PlatformStarted {
                version: "0.1".into(),
            },
        });
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::TaskCreated {
                title: "build-thing".into(),
                description: "...".into(),
            },
        });

        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();

        // 顶部 bar 包含 "events/s" 字段
        assert!(row_text(&buf, 0).contains("events/s"));
        // 底部 bar 的快捷键提示
        let last_y = buf.area.height - 1;
        let bottom = row_text(&buf, last_y);
        assert!(bottom.contains("q quit"));
        assert!(bottom.contains("filter"));

        // 中间区域能搜到事件 tag
        let middle: String = (1..last_y)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(middle.contains("platform_started"), "middle:\n{middle}");
        assert!(middle.contains("task_created"), "middle:\n{middle}");
        assert!(middle.contains("build-thing"), "middle:\n{middle}");
    }

    #[test]
    fn snapshot_render_hides_filtered_rows() {
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = FirehoseApp::new();
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::PlatformStarted {
                version: "0.1".into(),
            },
        });
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::TaskCreated {
                title: "t1".into(),
                description: "...".into(),
            },
        });

        // 输入过滤 "task"
        app.handle_key(KeyCode::Char('/'));
        for c in "task".chars() {
            app.handle_key(KeyCode::Char(c));
        }
        app.handle_key(KeyCode::Enter);

        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();

        let last_y = buf.area.height - 1;
        let middle: String = (1..last_y)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(middle.contains("task_created"), "{middle}");
        assert!(!middle.contains("platform_started"), "{middle}");
    }

    #[test]
    fn short_id_truncates_uuid_tail() {
        let s = "agent-12345678-9abc-def0-1234-56789abcdef0";
        assert_eq!(short_id(s).len(), 8);
    }

    #[test]
    fn one_line_truncates_and_collapses_newlines() {
        let s = "hello\nworld";
        assert_eq!(one_line(s, 20), "hello world");
        assert!(one_line(s, 4).ends_with('…'));
    }
}
