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
    /// 单行摘要。远端事件已带 `[<node_id>] ` 前缀。
    pub summary: String,
    /// 渲染颜色——按 kind 大类分配。
    pub color: Color,
    /// 远端事件的 source node_id；本地事件 None。渲染时据此叠 `Modifier::DIM`，
    /// 让用户秒级辨别"这条来自哪台机器"。
    pub source_node_id: Option<String>,
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
        let base_summary = summarize(&ev.kind);
        // 远端事件 summary 前缀 `[<node_id>] `——人眼第一眼就能区分本地/远端，
        // 比修改 color 信息更密（color 还在传达 kind 大类）。
        let summary = match &ev.meta.source_node_id {
            Some(node) => format!("[{node}] {base_summary}"),
            None => base_summary,
        };
        let color = color_for(&ev.kind);
        Self {
            time,
            who,
            kind_tag,
            summary,
            color,
            source_node_id: ev.meta.source_node_id.clone(),
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
                // 远端事件整行叠 DIM——保留原色（kind 大类语义）+ 暗一档（远端维度）。
                // 单行文本只 dim kind_tag 一段会让 summary 与 kind_tag 视觉脱节，
                // 索性整行（time/who/kind_tag/summary）都加 DIM 修饰。
                let dim = if r.source_node_id.is_some() {
                    Modifier::DIM
                } else {
                    Modifier::empty()
                };
                let line = Line::from(vec![
                    Span::styled(
                        r.time.clone(),
                        Style::default().fg(Color::DarkGray).add_modifier(dim),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("{:<12}", short(&r.who, 12)),
                        Style::default().fg(Color::Yellow).add_modifier(dim),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("{:<22}", r.kind_tag),
                        Style::default()
                            .fg(r.color)
                            .add_modifier(Modifier::BOLD | dim),
                    ),
                    Span::raw(" "),
                    Span::styled(r.summary.clone(), Style::default().add_modifier(dim)),
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
        TaskBlocked { reason } => format!("reason={reason}"),
        TaskResumed { input } => format!(
            "input={}",
            input
                .as_deref()
                .map(|s| one_line(s, 40))
                .unwrap_or_default()
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
        UserInterventionSent {
            target, mode, text, ..
        } => {
            // mentions 字段（v3 #N7'）TUI 不渲染——TUI 没 chip 视觉，
            // target 已显在前缀里；多 @ 信息留给 PWA。
            format!(
                "→{} [{}]: {}",
                short_id(&target.to_string()),
                mode,
                one_line(text, 40)
            )
        }
        AgentInterrupted { reason } => format!("interrupted: {reason}"),
        TaskInterventionApplied { mode } => format!("intervention applied [{mode}]"),
        OrchestratorCcReceived {
            from_user_to, text, ..
        } => format!(
            "cc {} {}",
            short_id(&from_user_to.to_string()),
            one_line(text, 40)
        ),
        TriggerRegistered { id, kind, .. } => format!("trigger+ {kind}:{id}"),
        TriggerFired { id, cause, .. } => format!("trigger! {id} [{cause}]"),
        TriggerDispatched { id, to_agent } => {
            format!("trigger→{} {id}", short_id(&to_agent.to_string()))
        }
        TriggerSkipped { id, reason } => format!("trigger~ {id}: {reason}"),
        TriggerFailed { id, error } => format!("trigger✗ {id}: {}", one_line(error, 40)),
        PlatformStarted { version } => format!("fuxi {version}"),
        PlatformStopping => "platform stopping".to_string(),
        SkillStaged { role, template, .. } => format!("role={role} template={template}"),
        SkillApproved { role } => format!("role={role}"),
        SkillRejected { role, reason } => format!("role={role} reason={}", one_line(reason, 40)),
        SkillActivated { role } => format!("role={role}"),
        NoRoleMatched { need } => format!("need={}", one_line(need, 50)),
        AgentRequestReview {
            agent,
            deliverable_kind,
            summary,
            ..
        } => format!(
            "review← {} [{:?}]: {}",
            short_id(&agent.to_string()),
            deliverable_kind,
            one_line(summary, 40)
        ),
        ReviewRequestTimeout {
            agent,
            waited_for_ms,
            ..
        } => format!(
            "review✗ {} timeout {}ms",
            short_id(&agent.to_string()),
            waited_for_ms
        ),
        WorkerRegistered {
            node_id,
            tags,
            max_concurrency,
        } => format!("worker+ {node_id} tags={tags:?} cap={max_concurrency}"),
        WorkerHeartbeatStateChanged {
            node_id,
            inflight_count,
            status,
        } => format!("worker~ {node_id} inflight={inflight_count} {status}"),
        WorkerStaleSwept {
            node_id,
            recycled_jobs,
        } => format!("worker✗ {node_id} recycled={}", recycled_jobs.len()),
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
        | TaskBlocked { .. }
        | TaskResumed { .. } => Color::Green,

        UserPrompted { .. } | AgentResponded { .. } | ThinkingStarted | ThinkingFinished => {
            Color::Cyan
        }

        ToolCallStarted { .. } | ToolCallFinished { .. } => Color::Blue,

        // WHY 单独抽出 AgentInterrupted 走 LightRed：
        // 打断是少见但极其重要的状态变更（M3.6 决定），用警告色和"调度家族"区分。
        AgentInterrupted { .. } => Color::LightRed,

        UserInterventionSent { .. }
        | TaskInterventionApplied { .. }
        | OrchestratorCcReceived { .. } => Color::Yellow,

        // deliverable 边界（Decision 13）—— review 请求是玄女 attention 唯一入口，
        // 用 LightYellow 与"介入家族"的 Yellow 区分；timeout 走 LightRed 与
        // AgentInterrupted 同级，提醒"该看了"。
        AgentRequestReview { .. } => Color::LightYellow,
        ReviewRequestTimeout { .. } => Color::LightRed,

        // 招贤一族 —— 醒目的红，因为是"生新 role"的高权限动作。
        SkillStaged { .. }
        | SkillApproved { .. }
        | SkillRejected { .. }
        | SkillActivated { .. }
        | NoRoleMatched { .. } => Color::Red,

        // 更漏一族 —— 同样醒目的红，都是时机性高权限事件。
        TriggerRegistered { .. }
        | TriggerFired { .. }
        | TriggerDispatched { .. }
        | TriggerSkipped { .. }
        | TriggerFailed { .. } => Color::Red,

        // 拓扑一族 —— register/heartbeat 平时事件用 Cyan（与"对话家族"的 Cyan
        // 视觉接近不冲突，都是"flow 状态"），sweep 用 LightRed 强调失联。
        WorkerRegistered { .. } | WorkerHeartbeatStateChanged { .. } => Color::Cyan,
        WorkerStaleSwept { .. } => Color::LightRed,

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

    /// M3.6：AgentInterrupted 是少见但极重要的状态变更，必须是 LightRed
    /// 警告色，不能再混在调度家族（Yellow）里被淹没。
    #[test]
    fn color_for_agent_interrupted_is_warning_red() {
        let kind = EventKind::AgentInterrupted {
            reason: "user".into(),
        };
        assert_eq!(color_for(&kind), Color::LightRed);
    }

    /// δ #4：远端事件 summary 必须以 `[<node_id>] ` 前缀开头。本地事件不带前缀。
    /// EventRow.source_node_id 字段需保留 raw node_id 供渲染层叠 DIM。
    #[test]
    fn event_row_remote_prefixes_summary_and_keeps_node_id() {
        let mut meta = EventMeta::now();
        meta.source_node_id = Some("home".into());
        let ev = Event {
            meta,
            kind: EventKind::AgentResponded {
                text: "远端响应".into(),
            },
        };
        let row = EventRow::from_event(&ev);
        assert!(
            row.summary.starts_with("[home] "),
            "summary={:?}",
            row.summary
        );
        assert_eq!(row.source_node_id.as_deref(), Some("home"));

        // 本地事件对照——无前缀，source_node_id None。
        let ev_local = Event {
            meta: EventMeta::now(),
            kind: EventKind::AgentResponded {
                text: "本地响应".into(),
            },
        };
        let row_local = EventRow::from_event(&ev_local);
        assert!(!row_local.summary.starts_with("["), "{}", row_local.summary);
        assert!(row_local.source_node_id.is_none());
    }

    /// δ #4：渲染时远端行的 kind_tag span 应叠加 `Modifier::DIM`——TestBackend
    /// 的 buffer cell 带 modifier 字段，可直接比对。本地行不带 DIM。
    #[test]
    fn snapshot_render_dims_remote_row_kind_tag() {
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = FirehoseApp::new();
        // 本地一条 + 远端一条。
        app.ingest(&Event {
            meta: EventMeta::now(),
            kind: EventKind::PlatformStarted {
                version: "0.1".into(),
            },
        });
        let mut remote_meta = EventMeta::now();
        remote_meta.source_node_id = Some("far".into());
        app.ingest(&Event {
            meta: remote_meta,
            kind: EventKind::AgentResponded {
                text: "远端来的".into(),
            },
        });

        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();

        // 找 "platform_started" 与 "agent_responded" 各自所在行；扫每行第一个非空格 cell
        // 之后查 kind_tag 列附近的 modifier。简化：分别在 row 1/2 (top bar 占 row 0)。
        let last_y = buf.area.height - 1;
        let mut local_dim_count = 0;
        let mut remote_dim_count = 0;
        for y in 1..last_y {
            let line = row_text(&buf, y);
            if line.contains("platform_started") {
                for x in 0..buf.area.width {
                    if buf[(x, y)].modifier.contains(Modifier::DIM) {
                        local_dim_count += 1;
                    }
                }
            } else if line.contains("agent_responded") {
                for x in 0..buf.area.width {
                    if buf[(x, y)].modifier.contains(Modifier::DIM) {
                        remote_dim_count += 1;
                    }
                }
            }
        }
        assert_eq!(local_dim_count, 0, "本地行不应有 DIM cell");
        assert!(
            remote_dim_count > 0,
            "远端行应至少有一个 DIM cell（kind_tag/summary 等）"
        );
    }

    /// δ #4：snapshot 上能直接看到 `[far] ` 前缀字样——人眼/grep 友好。
    #[test]
    fn snapshot_render_shows_remote_node_prefix() {
        let backend = TestBackend::new(80, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = FirehoseApp::new();
        let mut remote_meta = EventMeta::now();
        remote_meta.source_node_id = Some("far".into());
        app.ingest(&Event {
            meta: remote_meta,
            kind: EventKind::WorkerRegistered {
                node_id: "alpha".into(),
                tags: vec!["cc".into()],
                max_concurrency: 2,
            },
        });

        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();

        let last_y = buf.area.height - 1;
        let middle: String = (1..last_y)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            middle.contains("[far]"),
            "应渲染 [far] 前缀；middle:\n{middle}"
        );
        assert!(
            middle.contains("worker_registered"),
            "kind_tag 仍要在；middle:\n{middle}"
        );
    }
}
