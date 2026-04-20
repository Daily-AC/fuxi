//! `fuxi`（无参）—— v1 用户唯一入口：三栏 REPL TUI（任务树版）。
//!
//! 设计锚：
//! - `docs/architecture-v1.md` §M1.4（三栏 TUI · 任务树模型）
//! - `docs/research/tui-3pane-design.md`（骨架 + Key routing）
//!
//! 布局 `Layout::horizontal([Length(28), Min(40), Length(30)])`：
//!   - **左栏**：任务树（玄女顶部 → 活跃任务（挂对应门客） → 空闲门客），F2 折叠/展开事件流
//!   - **中栏**：vertical([Min(5), Length(3), Length(1)]) = 对话 + 输入 + 状态
//!   - **右栏**：task-level 元信息（title/worker/elapsed/最近工具调用）
//!
//! `active: ActiveTarget` 决定用户输入送给谁：
//!   - `Xuannv` → `Fuxi::dispatch(xuannv_id, new Task("user-turn", text))`
//!   - `Worker(id)` → `Fuxi::intervene(id, false, text)`（抄送给玄女）

use crate::daemon::Daemon;
use crate::ipc;
use anyhow::{Context, Result, anyhow};
use clap::Args as ClapArgs;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    MouseButton, MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::agent::AgentCard;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::{Task, TaskState};
use fuxi_core::trigger_lookup::TriggerLookup;
use fuxi_events::EventBus;
use fuxi_firehose::{EventRow, FirehoseApp, Hub};
use fuxi_memory::OracleStore;

use crate::click_registry::ClickRegistry;
use crate::theme::{self, Theme};

/// 鼠标点击落到哪里就触发哪个动作。v1 只三个粗粒度 pane 切换——
/// per-row 选中 / 按钮 hit-test 等留 v2。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // Focus* 共享前缀是有意的：语义就是"把焦点切到 X"
pub(crate) enum ClickAction {
    /// 点到左栏：把焦点切到任务树。
    FocusRoster,
    /// 点到中下输入框：把焦点切到输入（用户期望的默认态）。
    FocusInput,
    /// 点到中上对话区：焦点设为 Input 但不吞当前行——等 v2 做 per-msg 交互。
    FocusDialogue,
}

/// 全局主题——启动时一次从 env 读定，widget 内 `theme()` 取引用。
///
/// 为什么 `OnceLock` 而不是 struct field：theme 是**只读**的 presentation
/// 配置，把它塞到 ReplApp / 各 draw_* 方法会让大量函数签名污染。OnceLock
/// 让静态访问成 O(1) 指针 deref，多线程读也安全。v1 不支持运行时切主题
/// （重启 fuxi 才重读 FUXI_THEME），v2 若要可以 `RwLock<Theme>`。
static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();

pub(crate) fn theme() -> &'static Theme {
    THEME.get_or_init(theme::from_env)
}
use fuxi_orchestrator::{Fuxi, FuxiConfig, ShelfStatus, SystemEventBridge, WorkerKind};
use fuxi_scheduler::keeper::SystemClock;
use fuxi_scheduler::{Keeper, TriggerStore};
use fuxi_skills as skill_loader;
use fuxi_workspace::GitWorktreeWorkspace;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tui_textarea::{Input, TextArea};
use unicode_width::UnicodeWidthStr;

/// 每个对话桶最多保留多少行。
const DIALOGUE_CAP: usize = 500;
/// 每秒刷 UI 的键盘 poll 窗口。
const KEY_POLL: Duration = Duration::from_millis(50);
/// Done/Cancelled/Failed 后保留多少时间让用户看得到「完成」再 prune。
const TASK_PRUNE_DELAY: Duration = Duration::from_secs(5);
/// 右栏最近工具调用最多保留几条。
const RECENT_TOOLS_CAP: usize = 5;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// 覆盖 Hub HTTP 监听地址。默认同 `fuxi up`，便于并行 `fuxi watch` 外部观察。
    #[arg(long, default_value = "127.0.0.1:4100")]
    pub bind: SocketAddr,
    /// Unix socket 路径覆盖（给玄女的 Bash 工具用）。默认 `$FUXI_SOCK` / `/tmp/fuxi.sock`。
    #[arg(long)]
    pub sock_path: Option<PathBuf>,
    /// 工作区根（worktree 存哪里）。默认当前目录。
    #[arg(long, default_value = ".")]
    pub workspace_root: PathBuf,
    /// 门客是否分配 worktree。REPL 默认关掉——玄女当前只下发 Bash 命令，不写代码。
    #[arg(long, default_value_t = false)]
    pub allocate_worktree: bool,
    /// 玄女的 role（skills/<role>/SKILL.md）。默认 `xuannv`。
    #[arg(long, default_value = "xuannv")]
    pub xuannv_role: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:4100".parse().expect("static socket addr"),
            sock_path: None,
            workspace_root: PathBuf::from("."),
            allocate_worktree: false,
            xuannv_role: "xuannv".to_string(),
        }
    }
}

/// 在 PATH 中找指定 binary。`path_env` 抽出来是为了单测可以注入合成 PATH，
/// 不污染全进程环境变量（`std::env::set_var` 在多线程并发跑测试时不安全）。
pub fn find_in_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join(name);
        if !candidate.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&candidate)
                && meta.permissions().mode() & 0o111 != 0
            {
                return Some(candidate);
            }
        }
        #[cfg(not(unix))]
        {
            return Some(candidate);
        }
    }
    None
}

/// 入装预检：`fuxi` binary 必须在 PATH 中，否则玄女的 Bash 工具调 `fuxi ...` 会
/// `command not found`，整个 platform 失语。
pub fn require_fuxi_in_path(name: &str, path_env: Option<&OsStr>) -> Result<PathBuf> {
    if let Some(p) = find_in_path(name, path_env) {
        return Ok(p);
    }
    Err(anyhow!(
        "找不到 `{name}` 二进制（玄女的工具底座）。请先安装：\n\
         \n    ./scripts/install.sh\n\n\
         它会跑 `cargo install --path crates/fuxi-cli --force`，把 `fuxi` 装到 \
         `~/.cargo/bin/`。装完后 `which fuxi` 应返回路径，再重启 fuxi。"
    ))
}

pub async fn run(args: Args) -> Result<()> {
    require_fuxi_in_path("fuxi", std::env::var_os("PATH").as_deref())?;

    if skill_loader::skills_root().is_none() {
        return Err(anyhow!(
            "找不到 skills 目录：试过 $FUXI_SKILLS_DIR / git-root/skills / ./skills / $HOME/.fuxi/skills 都不在。\n\
             建议：export FUXI_SKILLS_DIR=/Users/e0_7/fuxi/skills  （或把 fuxi/skills 软链到 ~/.fuxi/skills）"
        ));
    }

    let bus = EventBus::with_memory_store()
        .await
        .context("创建内存 EventBus 失败")?;
    let workspace = Arc::new(GitWorktreeWorkspace::with_default_base(
        args.workspace_root.clone(),
    ));
    let fuxi = Arc::new(Fuxi::with_config(
        bus.clone(),
        workspace,
        FuxiConfig {
            allocate_worktree: args.allocate_worktree,
            ..Default::default()
        },
    ));

    let hub = Arc::new(Hub::new(bus.clone()));
    let app_router = fuxi_firehose::hub::router(hub);
    let hub_listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind {} 失败", args.bind))?;
    let hub_task = tokio::spawn(async move {
        if let Err(e) = axum::serve(hub_listener, app_router).await {
            tracing::warn!(error = %e, "hub serve 退出");
        }
    });

    let sock_path = args.sock_path.clone().unwrap_or_else(ipc::socket_path);
    // SAFETY: daemon::serve 自己会 parent-dir ensure + 清残留；这里只把路径传进 env
    // 让 cc 子进程继承到 $FUXI_SOCK
    unsafe {
        std::env::set_var("FUXI_SOCK", &sock_path);
    }
    let sched_store = TriggerStore::connect_memory()
        .await
        .context("创建 scheduler 内存库")?;
    let keeper = Arc::new(Keeper::new(
        sched_store.clone(),
        bus.clone(),
        Arc::new(SystemClock),
    ));
    let keeper_task = Arc::clone(&keeper).spawn();
    let daemon = Daemon::new(fuxi.clone(), bus.clone(), sched_store.clone(), keeper);
    let daemon_shutdown = daemon.shutdown_handle();
    let sock_for_task = sock_path.clone();
    let daemon_task = tokio::spawn(async move {
        if let Err(e) = daemon.serve(&sock_for_task).await {
            tracing::warn!(error = %e, "daemon serve 异常");
        }
    });

    let _ = bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    });

    let loaded = skill_loader::load(&args.xuannv_role)
        .with_context(|| format!("加载 skills/{}/SKILL.md", args.xuannv_role))?;
    let xuannv_profile = loaded.profile.clone();

    // 玄女 cc session 续写：策府存哪一个 session_id 就 `--resume` 它，没有则
    // 首次新生成并回写。这样关了 fuxi 再开，玄女上下文不丢。见 `session.rs`。
    let memory_db = crate::memory_cmd::resolve_db_path(None).context("解析策府 DB 路径")?;
    let oracle = OracleStore::connect_file(&memory_db)
        .await
        .with_context(|| format!("连接策府 DB {}", memory_db.display()))?;
    let (resume_session_id, session_id) = crate::session::resolve_xuannv_session(&oracle)
        .await
        .context("解析玄女 session_id")?;
    match (&resume_session_id, &session_id) {
        (Some(id), _) => tracing::info!(session = %id, "玄女 cc 续写策府已有 session"),
        (_, Some(id)) => tracing::info!(session = %id, "玄女 cc 首次启动，新 session_id 已落盘"),
        _ => unreachable!("resolve_xuannv_session 至少返回一个 Some"),
    }

    let cc_cfg = CcLaunchConfig {
        append_system_prompt: if loaded.append_system_prompt.is_empty() {
            None
        } else {
            Some(loaded.append_system_prompt)
        },
        allowed_tools: loaded.allowed_tools,
        resume_session_id,
        session_id,
        ..Default::default()
    };
    let xuannv_id = fuxi
        .spawn_worker(xuannv_profile, WorkerKind::Cc(cc_cfg))
        .await
        .context("玄女 spawn 失败")?;
    fuxi.set_xuannv(xuannv_id).await;
    tracing::info!(xuannv = %xuannv_id, "玄女已就绪");

    let trigger_lookup: Arc<dyn TriggerLookup> = Arc::new(sched_store.clone());
    let bridge_task =
        SystemEventBridge::spawn(fuxi.clone(), bus.clone(), xuannv_id, trigger_lookup);

    let greet = Task::new(
        "greet",
        "用户刚启动 fuxi REPL。请用一句话（十字以内）主动问好，邀请用户提需求。不要自我介绍。",
    );
    if let Err(e) = fuxi.dispatch(xuannv_id, greet).await {
        tracing::warn!(error = %e, "greet dispatch 失败，继续");
    }

    let outcome = drive_tui(bus, fuxi.clone(), xuannv_id).await;

    daemon_shutdown.notify_waiters();
    if let Err(e) = fuxi.shutdown().await {
        tracing::warn!(error = %e, "fuxi shutdown 部分失败");
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    hub_task.abort();
    daemon_task.abort();
    keeper_task.abort();
    bridge_task.abort();

    outcome
}

/// 键盘焦点——v1 只两态；事件流折叠靠 F2，不抢焦点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Roster,
    Input,
}

/// 主对话对象——对谁说话、右栏展示谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ActiveTarget {
    Xuannv,
    Worker(AgentId),
}

/// 一条对话行。
#[derive(Debug, Clone)]
pub(crate) enum DialogueLine {
    User(String),
    /// agent 自称——玄女 / 门客都用这种。name 用来画前缀。
    Agent {
        name: String,
        text: String,
    },
    System(String),
}

/// 门客（roster 卡片）。
#[derive(Debug, Clone)]
pub(crate) struct RosterRow {
    pub id: AgentId,
    pub role: String,
    pub name: String,
    pub status: ShelfStatus,
}

/// 任务节点——左栏任务树的基本单位。
#[derive(Debug, Clone)]
pub(crate) struct TaskNode {
    pub task_id: TaskId,
    pub title: String,
    pub description: String,
    pub state: TaskState,
    pub worker: AgentId,
    pub worker_role: String,
    pub dispatched_at: Instant,
    /// 完成/取消/失败后 5s prune；None 代表仍活跃。
    pub prune_after: Option<Instant>,
    pub thinking: bool,
    pub worktree: Option<PathBuf>,
    /// 最近工具调用摘要 `tool=args前40字`，右栏展示。
    pub recent_tools: VecDeque<String>,
}

/// 左栏扁平行——用于渲染 + `roster_state` 选中计算。
#[derive(Debug, Clone, Copy)]
pub(crate) enum PaneRow {
    Xuannv,
    Task(usize),
    /// 空闲门客分组标题（不可选）。
    IdleHeader,
    Idle(usize),
}

/// 用户按 Enter 后计算出的提交意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Submit {
    Xuannv(String),
    Worker(AgentId, String),
}

/// REPL TUI 的核心状态。纯逻辑，不 own terminal——便于单测。
pub(crate) struct ReplApp {
    pub(crate) xuannv_id: AgentId,
    pub(crate) xuannv_status: ShelfStatus,
    pub(crate) xuannv_thinking: bool,

    pub(crate) focus: Focus,
    pub(crate) active: ActiveTarget,
    pub(crate) dialogues: HashMap<ActiveTarget, VecDeque<DialogueLine>>,

    pub(crate) tasks: Vec<TaskNode>,
    pub(crate) idle_workers: Vec<RosterRow>,

    pub(crate) roster_state: ListState,
    pub(crate) events_visible: bool,
    pub(crate) events: FirehoseApp,

    pub(crate) input: TextArea<'static>,

    /// 对话滚动：`auto_scroll=true` 贴底；用户 PgUp 后置 false 冻结。
    pub(crate) dialogue_scroll: u16,
    pub(crate) dialogue_auto_scroll: bool,
    pub(crate) last_dialogue_total: u16,
    pub(crate) last_dialogue_view: u16,

    pub(crate) should_quit: bool,
    pub(crate) confirm_quit: bool,

    /// 任务 prune 延迟——测试里可调短。
    pub(crate) prune_delay: Duration,

    /// 鼠标点击区注册表。每帧 `draw()` 开头 `clear()`，各 draw_*
    /// 末尾 `register(area, ClickAction::Xxx)`。mouse 事件 hit_test 分派。
    pub(crate) click: ClickRegistry<ClickAction>,
}

fn new_textarea() -> TextArea<'static> {
    let mut ta = TextArea::default();
    ta.set_cursor_line_style(Style::default());
    ta.set_placeholder_text("输入（Shift+Enter / Alt+Enter / Ctrl+J 换行；Enter 发送）");
    ta
}

impl ReplApp {
    pub(crate) fn new(xuannv_id: AgentId) -> Self {
        let mut app = Self {
            xuannv_id,
            xuannv_status: ShelfStatus::Idle,
            xuannv_thinking: false,
            focus: Focus::Input,
            active: ActiveTarget::Xuannv,
            dialogues: HashMap::new(),
            tasks: Vec::new(),
            idle_workers: Vec::new(),
            roster_state: ListState::default(),
            events_visible: false,
            events: FirehoseApp::new(),
            input: new_textarea(),
            dialogue_scroll: 0,
            dialogue_auto_scroll: true,
            last_dialogue_total: 0,
            last_dialogue_view: 0,
            should_quit: false,
            confirm_quit: false,
            prune_delay: TASK_PRUNE_DELAY,
            click: ClickRegistry::new(),
        };
        app.roster_state.select(Some(0));
        app
    }

    /// 构造仅含玄女的 app——测试帮手。
    #[cfg(test)]
    fn stub() -> Self {
        Self::new(AgentId::new())
    }

    /// 当前输入文本（`textarea.lines()` 按换行拼回）。
    pub(crate) fn input_text(&self) -> String {
        self.input.lines().join("\n")
    }

    /// 粘贴事件 → 全塞进 textarea。
    /// 公理：bracketed paste 让 IME / 剪贴板整块内容一次进入，避免逐键 race。
    pub(crate) fn handle_paste(&mut self, s: &str) {
        self.focus = Focus::Input;
        self.input.insert_str(s);
    }

    pub(crate) fn push_line(&mut self, target: ActiveTarget, line: DialogueLine) {
        let bucket = self.dialogues.entry(target).or_default();
        if bucket.len() == DIALOGUE_CAP {
            bucket.pop_front();
        }
        bucket.push_back(line);
    }

    /// 左栏扁平行——按 `Xuannv → tasks... → IdleHeader → idle_workers...` 顺序。
    /// IdleHeader 在没有空闲门客时不出现。
    pub(crate) fn pane_rows(&self) -> Vec<PaneRow> {
        let mut rows = Vec::with_capacity(2 + self.tasks.len() + self.idle_workers.len());
        rows.push(PaneRow::Xuannv);
        for i in 0..self.tasks.len() {
            rows.push(PaneRow::Task(i));
        }
        if !self.idle_workers.is_empty() {
            rows.push(PaneRow::IdleHeader);
            for i in 0..self.idle_workers.len() {
                rows.push(PaneRow::Idle(i));
            }
        }
        rows
    }

    /// 事件总线摄入——分路到对话桶 / 任务树 / 事件流。
    pub(crate) fn ingest(&mut self, ev: &Event) {
        self.events.ingest(ev);
        let who = ev.meta.agent;
        let xuannv = self.xuannv_id;

        #[inline]
        fn tgt(x: AgentId, id: AgentId) -> ActiveTarget {
            if id == x {
                ActiveTarget::Xuannv
            } else {
                ActiveTarget::Worker(id)
            }
        }

        match &ev.kind {
            EventKind::AgentSpawning { role, .. } => {
                if let Some(id) = who {
                    self.upsert_idle(id, role.clone(), ShelfStatus::Idle);
                }
            }
            EventKind::AgentReady { .. } => {
                if let Some(id) = who {
                    self.set_agent_status(id, ShelfStatus::Idle);
                }
            }
            EventKind::AgentDead { cause } => {
                if let Some(id) = who {
                    self.push_line(
                        tgt(xuannv, id),
                        DialogueLine::System(format!("⚠ 下线：{cause}")),
                    );
                    self.handle_agent_dead(id);
                }
            }
            EventKind::UserPrompted { text } => {
                self.push_line(self.active, DialogueLine::User(text.clone()));
            }
            EventKind::UserInterventionSent { target, text, .. } => {
                self.push_line(
                    ActiveTarget::Worker(*target),
                    DialogueLine::User(text.clone()),
                );
            }
            EventKind::AgentResponded { text } => {
                if let Some(id) = who {
                    let name = self.agent_display_name(id);
                    self.push_line(
                        tgt(xuannv, id),
                        DialogueLine::Agent {
                            name,
                            text: text.clone(),
                        },
                    );
                }
            }
            EventKind::ThinkingStarted => {
                if let Some(id) = who {
                    self.set_thinking(id, true);
                }
            }
            EventKind::ThinkingFinished => {
                if let Some(id) = who {
                    self.set_thinking(id, false);
                }
            }
            EventKind::TaskDispatched { to } => {
                if let Some(tid) = ev.meta.task {
                    let role = self.lookup_role(*to);
                    self.upsert_task(tid, *to, role);
                }
            }
            EventKind::TaskCreated { title, description } => {
                if let (Some(id), Some(tid)) = (who, ev.meta.task) {
                    let role = self.lookup_role(id);
                    self.upsert_task(tid, id, role);
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.task_id == tid) {
                        t.title = title.clone();
                        t.description = description.clone();
                    }
                }
            }
            EventKind::TaskStateChanged { to, .. } => {
                if let Some(tid) = ev.meta.task {
                    let delay = self.prune_delay;
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.task_id == tid) {
                        t.state = *to;
                        if matches!(to, TaskState::Done | TaskState::Cancelled) {
                            t.prune_after = Some(Instant::now() + delay);
                        }
                    }
                }
            }
            EventKind::TaskDelivered { .. } | EventKind::TaskCancelled { .. } => {
                if let Some(tid) = ev.meta.task {
                    let delay = self.prune_delay;
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.task_id == tid) {
                        t.prune_after = Some(Instant::now() + delay);
                    }
                }
            }
            EventKind::ToolCallStarted { tool, args } => {
                if let Some(id) = who.filter(|i| *i != xuannv) {
                    let summary = tool_arg_preview(tool, args);
                    if let Some(t) = self.task_by_worker_mut(id) {
                        if t.recent_tools.len() >= RECENT_TOOLS_CAP {
                            t.recent_tools.pop_front();
                        }
                        t.recent_tools.push_back(summary);
                    }
                }
            }
            EventKind::ConversationHandoffRequested { to, .. } => {
                self.active = tgt(xuannv, *to);
                self.focus = Focus::Input;
                self.resync_roster_selection();
            }
            _ => {}
        }
    }

    /// 完成/取消后清理过期任务。由 drive_tui 每帧前调一次；测试里直接喂 Instant。
    pub(crate) fn tick(&mut self, now: Instant) {
        let before = self.tasks.len();
        let mut freed = Vec::new();
        self.tasks.retain(|t| match t.prune_after {
            Some(after) if now >= after => {
                freed.push((t.worker, t.worker_role.clone()));
                false
            }
            _ => true,
        });
        // prune 掉的 worker 回空闲桶（如果它还在——AgentDead 已清就别回）
        for (wid, role) in freed {
            if wid != self.xuannv_id
                && !self.tasks.iter().any(|t| t.worker == wid)
                && !self.idle_workers.iter().any(|r| r.id == wid)
            {
                self.idle_workers.push(RosterRow {
                    id: wid,
                    role: role.clone(),
                    name: role,
                    status: ShelfStatus::Idle,
                });
            }
        }
        if before != self.tasks.len() {
            if let ActiveTarget::Worker(id) = self.active
                && !self.tasks.iter().any(|t| t.worker == id)
                && !self.idle_workers.iter().any(|r| r.id == id)
            {
                self.active = ActiveTarget::Xuannv;
            }
            self.resync_roster_selection();
        }
    }

    fn handle_agent_dead(&mut self, id: AgentId) {
        if id == self.xuannv_id {
            self.xuannv_status = ShelfStatus::Dead;
            return;
        }
        // 从空闲桶移除
        self.idle_workers.retain(|r| r.id != id);
        // 活跃任务 → 标 prune
        let delay = self.prune_delay;
        let now = Instant::now();
        for t in self.tasks.iter_mut().filter(|t| t.worker == id) {
            if t.prune_after.is_none() {
                t.prune_after = Some(now + delay);
            }
        }
    }

    fn upsert_task(&mut self, task_id: TaskId, worker: AgentId, role: String) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.task_id == task_id) {
            t.worker = worker;
            t.worker_role = role;
            t.prune_after = None;
            return;
        }
        let node = TaskNode {
            task_id,
            title: String::new(),
            description: String::new(),
            state: TaskState::New,
            worker,
            worker_role: role,
            dispatched_at: Instant::now(),
            prune_after: None,
            thinking: false,
            worktree: None,
            recent_tools: VecDeque::with_capacity(RECENT_TOOLS_CAP),
        };
        self.tasks.push(node);
        // 玄女自己接 task 不影响 idle 桶；门客挂了 task 从空闲移走
        if worker != self.xuannv_id {
            self.idle_workers.retain(|r| r.id != worker);
        }
    }

    fn upsert_idle(&mut self, id: AgentId, role: String, status: ShelfStatus) {
        if id == self.xuannv_id {
            self.xuannv_status = status;
            return;
        }
        // 已挂任务就不加到空闲桶（task_dispatched 优先）
        if self
            .tasks
            .iter()
            .any(|t| t.worker == id && t.prune_after.is_none())
        {
            return;
        }
        if let Some(r) = self.idle_workers.iter_mut().find(|r| r.id == id) {
            r.role = role;
            r.status = status;
        } else {
            self.idle_workers.push(RosterRow {
                id,
                role: role.clone(),
                name: role,
                status,
            });
        }
    }

    fn set_agent_status(&mut self, id: AgentId, status: ShelfStatus) {
        if id == self.xuannv_id {
            self.xuannv_status = status;
            return;
        }
        if let Some(r) = self.idle_workers.iter_mut().find(|r| r.id == id) {
            r.status = status;
        }
    }

    fn set_thinking(&mut self, id: AgentId, flag: bool) {
        if id == self.xuannv_id {
            self.xuannv_thinking = flag;
            return;
        }
        if let Some(t) = self.task_by_worker_mut(id) {
            t.thinking = flag;
        }
    }

    fn task_by_worker_mut(&mut self, id: AgentId) -> Option<&mut TaskNode> {
        self.tasks
            .iter_mut()
            .filter(|t| t.worker == id && t.prune_after.is_none())
            .max_by_key(|t| t.dispatched_at)
    }

    fn lookup_role(&self, id: AgentId) -> String {
        if id == self.xuannv_id {
            return "xuannv".into();
        }
        if let Some(r) = self.idle_workers.iter().find(|r| r.id == id) {
            return r.role.clone();
        }
        if let Some(t) = self.tasks.iter().find(|t| t.worker == id) {
            return t.worker_role.clone();
        }
        "worker".into()
    }

    fn agent_display_name(&self, id: AgentId) -> String {
        if id == self.xuannv_id {
            return "玄女".into();
        }
        self.lookup_role(id)
    }

    fn resync_roster_selection(&mut self) {
        let rows = self.pane_rows();
        let want = match self.active {
            ActiveTarget::Xuannv => rows.iter().position(|r| matches!(r, PaneRow::Xuannv)),
            ActiveTarget::Worker(id) => rows.iter().position(|r| match r {
                PaneRow::Task(i) => self.tasks[*i].worker == id,
                PaneRow::Idle(i) => self.idle_workers[*i].id == id,
                _ => false,
            }),
        };
        self.roster_state.select(want.or(Some(0)));
    }

    /// Tab 循环切 active：Xuannv → tasks[0].worker → tasks[1].worker → ... → idle[0] → ...
    /// 跳过 IdleHeader（非选择项）。
    pub(crate) fn cycle_active_to_next(&mut self) {
        let rows = self.pane_rows();
        let selectable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !matches!(r, PaneRow::IdleHeader))
            .map(|(i, _)| i)
            .collect();
        if selectable.is_empty() {
            return;
        }
        let cur = self.current_row_index(&rows);
        let cur_pos = selectable.iter().position(|&i| i == cur).unwrap_or(0);
        let next_pos = (cur_pos + 1) % selectable.len();
        self.select_row_at(&rows, selectable[next_pos]);
    }

    fn current_row_index(&self, rows: &[PaneRow]) -> usize {
        match self.active {
            ActiveTarget::Xuannv => rows
                .iter()
                .position(|r| matches!(r, PaneRow::Xuannv))
                .unwrap_or(0),
            ActiveTarget::Worker(id) => rows
                .iter()
                .position(|r| match r {
                    PaneRow::Task(i) => self.tasks[*i].worker == id,
                    PaneRow::Idle(i) => self.idle_workers[*i].id == id,
                    _ => false,
                })
                .unwrap_or(0),
        }
    }

    fn select_row_at(&mut self, rows: &[PaneRow], idx: usize) {
        if let Some(row) = rows.get(idx) {
            match row {
                PaneRow::Xuannv => self.active = ActiveTarget::Xuannv,
                PaneRow::Task(i) => {
                    self.active = ActiveTarget::Worker(self.tasks[*i].worker);
                }
                PaneRow::Idle(i) => {
                    self.active = ActiveTarget::Worker(self.idle_workers[*i].id);
                }
                PaneRow::IdleHeader => return,
            }
            self.roster_state.select(Some(idx));
        }
    }

    /// Esc：速回玄女。
    pub(crate) fn reset_to_xuannv(&mut self) {
        self.active = ActiveTarget::Xuannv;
        self.focus = Focus::Input;
        self.resync_roster_selection();
    }

    fn roster_up(&mut self) {
        let rows = self.pane_rows();
        if rows.is_empty() {
            return;
        }
        let cur = self.roster_state.selected().unwrap_or(0);
        let mut next = cur.saturating_sub(1);
        // 跳过 IdleHeader
        while matches!(rows.get(next), Some(PaneRow::IdleHeader)) && next > 0 {
            next -= 1;
        }
        self.roster_state.select(Some(next));
    }

    fn roster_down(&mut self) {
        let rows = self.pane_rows();
        if rows.is_empty() {
            return;
        }
        let cur = self.roster_state.selected().unwrap_or(0);
        let mut next = (cur + 1).min(rows.len() - 1);
        while matches!(rows.get(next), Some(PaneRow::IdleHeader)) && next + 1 < rows.len() {
            next += 1;
        }
        self.roster_state.select(Some(next));
    }

    fn roster_enter(&mut self) {
        let rows = self.pane_rows();
        let Some(idx) = self.roster_state.selected() else {
            return;
        };
        self.select_row_at(&rows, idx);
        self.focus = Focus::Input;
    }

    /// 处理一次按键。返回 Some(Submit) 表示有待提交意图；否则 None。
    pub(crate) fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<Submit> {
        if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
            if self.confirm_quit {
                self.should_quit = true;
            } else {
                self.confirm_quit = true;
                self.push_line(
                    self.active,
                    DialogueLine::System("再按一次 Ctrl-C 退出".into()),
                );
            }
            return None;
        }
        self.confirm_quit = false;

        // 全局键
        match code {
            KeyCode::Tab => {
                self.cycle_active_to_next();
                self.focus = Focus::Input;
                return None;
            }
            KeyCode::Esc => {
                self.reset_to_xuannv();
                return None;
            }
            KeyCode::F(2) => {
                self.events_visible = !self.events_visible;
                return None;
            }
            KeyCode::PageUp => {
                self.scroll_up_page();
                return None;
            }
            KeyCode::PageDown => {
                self.scroll_down_page();
                return None;
            }
            KeyCode::End if self.input_text().is_empty() => {
                self.dialogue_auto_scroll = true;
                self.dialogue_scroll = self
                    .last_dialogue_total
                    .saturating_sub(self.last_dialogue_view);
                return None;
            }
            KeyCode::Home if self.input_text().is_empty() => {
                self.dialogue_auto_scroll = false;
                self.dialogue_scroll = 0;
                return None;
            }
            _ => {}
        }

        if self.focus == Focus::Roster {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.roster_up();
                    return None;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.roster_down();
                    return None;
                }
                KeyCode::Enter => {
                    self.roster_enter();
                    return None;
                }
                KeyCode::Char(c) if !c.is_control() && c != '\t' => {
                    self.focus = Focus::Input;
                    self.input.insert_char(c);
                    return None;
                }
                _ => return None,
            }
        }

        // Focus::Input —— tui-textarea 路由
        //
        // 换行键三路兜底：Shift+Enter / Alt+Enter / Ctrl+J。理由：
        // - Shift+Enter 仅在终端开了 Kitty keyboard protocol（iTerm2/Ghostty/
        //   Kitty/Alacritty）时才带 SHIFT modifier；macOS 自带 Terminal.app 等
        //   老终端把它发成裸 Enter，handler 区分不出，只能以 Alt+Enter / Ctrl+J 兜底
        // - Ctrl+J 物理上就是 `\n`（0x0A），任何终端都认
        match code {
            KeyCode::Enter
                if mods.contains(KeyModifiers::SHIFT) || mods.contains(KeyModifiers::ALT) =>
            {
                self.input.insert_newline();
                None
            }
            KeyCode::Char('j') if mods == KeyModifiers::CONTROL => {
                self.input.insert_newline();
                None
            }
            KeyCode::Enter => {
                let text = self.take_input();
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                match self.active {
                    ActiveTarget::Xuannv => Some(Submit::Xuannv(trimmed.to_string())),
                    ActiveTarget::Worker(id) => Some(Submit::Worker(id, trimmed.to_string())),
                }
            }
            KeyCode::Char(c) if c.is_control() || c == '\t' => None,
            _ => {
                let event = KeyEvent::new(code, mods);
                self.input.input(Input::from(event));
                None
            }
        }
    }

    fn take_input(&mut self) -> String {
        let text = self.input.lines().join("\n");
        self.input = new_textarea();
        text
    }

    fn scroll_up_page(&mut self) {
        self.dialogue_auto_scroll = false;
        let step = self.last_dialogue_view.max(1);
        self.dialogue_scroll = self.dialogue_scroll.saturating_sub(step);
    }

    fn scroll_down_page(&mut self) {
        let max = self
            .last_dialogue_total
            .saturating_sub(self.last_dialogue_view);
        let step = self.last_dialogue_view.max(1);
        let new_scroll = self.dialogue_scroll.saturating_add(step).min(max);
        self.dialogue_scroll = new_scroll;
        if new_scroll >= max {
            self.dialogue_auto_scroll = true;
        }
    }

    pub(crate) fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        // 新一帧：清空上帧的 click regions（hit-test 后者胜，所以最后 register
        // 的 pane 会命中——pane 区域互不重叠，谁先注册其实无关）。
        self.click.clear();

        let root = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(28),
                Constraint::Min(40),
                Constraint::Length(30),
            ])
            .split(f.area());

        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),
                Constraint::Length(if self.events_visible { 10 } else { 0 }),
            ])
            .split(root[0]);
        self.draw_roster(f, left[0]);
        self.click.register(left[0], ClickAction::FocusRoster);
        if self.events_visible {
            self.draw_events(f, left[1]);
        }

        let center = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(root[1]);
        self.draw_dialogue(f, center[0]);
        self.click.register(center[0], ClickAction::FocusDialogue);
        self.draw_input(f, center[1]);
        self.click.register(center[1], ClickAction::FocusInput);
        self.draw_status(f, center[2]);

        self.draw_meta(f, root[2]);
    }

    /// 处理鼠标事件。v1 仅三类：
    ///
    /// - 左键按下 → `click.hit_test` → 切 pane focus
    /// - 滚轮 → 对话区滚动（与 PgUp/PgDn 行为一致）
    ///
    /// 其他（Drag / Moved / Right / Middle）先不处理。
    pub(crate) fn handle_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up_page();
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down_page();
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.click.hit_test(ev.column, ev.row).copied() {
                    self.apply_click(action);
                }
            }
            _ => {}
        }
    }

    fn apply_click(&mut self, a: ClickAction) {
        match a {
            ClickAction::FocusRoster => {
                self.focus = Focus::Roster;
            }
            ClickAction::FocusInput | ClickAction::FocusDialogue => {
                // 对话区点击暂等同"准备输入"——比把焦点悬空在对话上更符合用户直觉
                self.focus = Focus::Input;
            }
        }
    }

    fn draw_roster(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rows = self.pane_rows();
        let selected = self.roster_state.selected();
        let active_row_idx = self.current_row_index(&rows);
        let items: Vec<ListItem> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| match row {
                PaneRow::Xuannv => {
                    let marker = status_marker(self.xuannv_status);
                    let active_mark =
                        if active_row_idx == i && matches!(self.active, ActiveTarget::Xuannv) {
                            "▶ "
                        } else {
                            "  "
                        };
                    ListItem::new(Line::from(vec![
                        Span::raw(active_mark),
                        Span::raw(marker),
                        Span::raw(" "),
                        Span::styled(
                            "玄女",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" "),
                        Span::styled("总控", Style::default().fg(Color::DarkGray)),
                    ]))
                }
                PaneRow::Task(idx) => {
                    let t = &self.tasks[*idx];
                    let marker = if t.prune_after.is_some() {
                        "✓"
                    } else {
                        status_marker(task_state_to_shelf(t.state, t.thinking))
                    };
                    let active_mark = if active_row_idx == i
                        && matches!(self.active, ActiveTarget::Worker(w) if w == t.worker)
                    {
                        "▶ "
                    } else {
                        "  "
                    };
                    let title = if t.title.is_empty() {
                        "任务".to_string()
                    } else {
                        t.title.clone()
                    };
                    let state_color = if t.prune_after.is_some() {
                        Color::DarkGray
                    } else {
                        Color::White
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw(active_mark),
                        Span::raw("📁 "),
                        Span::styled(
                            truncate_by_width(&title, 16),
                            Style::default().fg(state_color),
                        ),
                        Span::raw("  "),
                        Span::raw(marker),
                        Span::raw(" "),
                        Span::styled(
                            truncate_by_width(&t.worker_role, 6),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                }
                PaneRow::IdleHeader => ListItem::new(Line::from(vec![Span::styled(
                    "─ 空闲门客 ─",
                    Style::default().fg(Color::DarkGray),
                )])),
                PaneRow::Idle(idx) => {
                    let r = &self.idle_workers[*idx];
                    let marker = status_marker(r.status);
                    let active_mark = if active_row_idx == i
                        && matches!(self.active, ActiveTarget::Worker(w) if w == r.id)
                    {
                        "▶ "
                    } else {
                        "  "
                    };
                    let style = if r.status == ShelfStatus::Dead {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw(active_mark),
                        Span::raw(marker),
                        Span::raw(" "),
                        Span::styled(truncate_by_width(&r.name, 8), style),
                        Span::raw(" "),
                        Span::styled(
                            truncate_by_width(&r.role, 8),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                }
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if self.focus == Focus::Roster {
                " ▸ 任务 "
            } else {
                " 任务 "
            })
            .border_style(Style::default().fg(if self.focus == Focus::Roster {
                theme().focus_border()
            } else {
                theme().dim_border()
            }));
        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(theme().muted())
                .add_modifier(Modifier::BOLD),
        );
        let _ = selected;
        f.render_stateful_widget(list, area, &mut self.roster_state);
    }

    fn draw_events(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let t = theme();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 事件（过滤噪声） ")
            .border_style(Style::default().fg(t.dim_border()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let rows = self.events.visible_rows();
        let filtered: Vec<_> = rows
            .iter()
            .filter(|r| !is_noise_event(r.kind_tag, &r.summary))
            .collect();
        let available = inner.height as usize;
        let start = filtered.len().saturating_sub(available);
        let lines: Vec<Line> = filtered[start..]
            .iter()
            .map(|r| {
                let (icon, color, narrative) = narrate_event(r);
                // 时间 (8) + icon (2) + who (6) + 3 空格 = 19 预留给头部
                let reserved = 20u16;
                let narrative_width = inner.width.saturating_sub(reserved).max(10) as usize;
                Line::from(vec![
                    Span::styled(r.time.clone(), Style::default().fg(t.muted())),
                    Span::raw(" "),
                    Span::styled(
                        icon,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(short_str(&r.who, 6), Style::default().fg(t.info())),
                    Span::raw(" "),
                    Span::styled(
                        truncate_by_width(&narrative, narrative_width),
                        Style::default().fg(color),
                    ),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn draw_dialogue(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let (active_label, thinking) = match self.active {
            ActiveTarget::Xuannv => ("玄女".to_string(), self.xuannv_thinking),
            ActiveTarget::Worker(id) => {
                let thinking = self
                    .tasks
                    .iter()
                    .find(|t| t.worker == id)
                    .map(|t| t.thinking)
                    .unwrap_or(false);
                (self.agent_display_name(id), thinking)
            }
        };
        let title = if thinking {
            format!(" {active_label}（思考中…） ")
        } else {
            format!(" {active_label} ")
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(theme().focus_border()));

        let empty = VecDeque::new();
        let bucket = self.dialogues.get(&self.active).unwrap_or(&empty);
        let lines: Vec<Line> = render_dialogue_collapsed(bucket.iter());

        let inner_h = area.height.saturating_sub(2);
        let total = lines.len() as u16;
        self.last_dialogue_total = total;
        self.last_dialogue_view = inner_h;
        if self.dialogue_auto_scroll {
            self.dialogue_scroll = total.saturating_sub(inner_h);
        } else {
            let max = total.saturating_sub(inner_h);
            if self.dialogue_scroll > max {
                self.dialogue_scroll = max;
            }
        }

        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.dialogue_scroll, 0));
        f.render_widget(para, area);
    }

    fn draw_input(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let prefix = match self.active {
            ActiveTarget::Xuannv => "玄女> ".to_string(),
            ActiveTarget::Worker(id) => format!("{}> ", self.agent_display_name(id)),
        };
        let title = format!(" 你 → {prefix}");
        let mut ta_widget = self.input.clone();
        ta_widget.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(if self.focus == Focus::Input {
                    theme().success()
                } else {
                    theme().dim_border()
                })),
        );
        f.render_widget(&ta_widget, area);
    }

    fn draw_status(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let hint = " Tab 循环 | Esc 回玄女 | F2 事件流 | PgUp/PgDn 翻阅 | ⇧/⌥-Enter / ⌃-J 换行 | Enter 发送 | Ctrl-C 退出 ";
        let para = Paragraph::new(hint).style(Style::default().fg(Color::Black).bg(Color::Gray));
        f.render_widget(para, area);
    }

    fn draw_meta(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let (lines, title) = match self.active {
            ActiveTarget::Xuannv => {
                let task_line = if let Some(t) = self
                    .tasks
                    .iter()
                    .find(|t| t.worker == self.xuannv_id && t.prune_after.is_none())
                {
                    t.title.clone()
                } else {
                    "-".into()
                };
                (
                    vec![
                        Line::from(format!("agent    {}", short_id_of(self.xuannv_id))),
                        Line::from(format!("status   {:?}", self.xuannv_status)),
                        Line::from(format!("active   {}", truncate_by_width(&task_line, 20))),
                        Line::from(format!("tasks    {}", self.tasks.len())),
                        Line::from(format!("idle     {}", self.idle_workers.len())),
                    ],
                    " 玄女 · 总控 ",
                )
            }
            ActiveTarget::Worker(id) => {
                if let Some(t) = self.tasks.iter().find(|t| t.worker == id) {
                    let mut lines = vec![
                        Line::from(format!("task     {}", truncate_by_width(&t.title, 20))),
                        Line::from(format!(
                            "desc     {}",
                            truncate_by_width(&t.description, 20)
                        )),
                        Line::from(format!("worker   {}", short_id_of(t.worker))),
                        Line::from(format!(
                            "role     {}",
                            truncate_by_width(&t.worker_role, 16)
                        )),
                        Line::from(format!("state    {:?}", t.state)),
                        Line::from(format!(
                            "elapsed  {}",
                            humanize_elapsed(t.dispatched_at.elapsed())
                        )),
                        Line::from(format!(
                            "worktree {}",
                            t.worktree
                                .as_ref()
                                .map(|p| p.display().to_string())
                                .unwrap_or("-".into())
                        )),
                    ];
                    if !t.recent_tools.is_empty() {
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(
                            "最近工具调用:",
                            Style::default().fg(Color::DarkGray),
                        )));
                        for s in t.recent_tools.iter().rev().take(3) {
                            lines.push(Line::from(format!("  · {}", truncate_by_width(s, 22))));
                        }
                    }
                    (lines, " 任务 · 元信息 ")
                } else if let Some(r) = self.idle_workers.iter().find(|r| r.id == id) {
                    (
                        vec![
                            Line::from(format!("worker   {}", short_id_of(r.id))),
                            Line::from(format!("role     {}", truncate_by_width(&r.role, 16))),
                            Line::from(format!("status   {:?}", r.status)),
                            Line::from("task     -"),
                            Line::from(Span::styled(
                                "（空闲中，等玄女派活）",
                                Style::default().fg(Color::DarkGray),
                            )),
                        ],
                        " 空闲门客 · 元信息 ",
                    )
                } else {
                    (vec![Line::from("（已下线）")], " 元信息 ")
                }
            }
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

/// 对话渲染。
///
/// 前缀策略（原来的 `你>` / `玄女>` 字面前缀已取消——输入框已经告诉用户
/// "你→玄女"，对话区再重复是废话，参考 Claude Code / Codex 的视觉）：
/// - **User**：每行左挂 `▍ ` teal 竖条，让多行消息视觉上成一块"气泡"
/// - **Agent**：首条首行 `● 名字 `，圆点 mauve + 名字 dim；同 speaker 相邻
///   消息或多行消息 subsequent 行只缩进对齐，不重复名字——对话视觉更紧凑
/// - **System**：`· ` 前缀 + italic，弱存在感
fn render_dialogue_collapsed<'a, I>(iter: I) -> Vec<Line<'a>>
where
    I: IntoIterator<Item = &'a DialogueLine>,
{
    let th = theme();
    let mut out = Vec::new();
    let mut prev_speaker: Option<String> = None;
    for line in iter {
        match line {
            DialogueLine::User(t) => {
                for ln in t.lines() {
                    out.push(Line::from(vec![
                        Span::styled("▍ ", Style::default().fg(th.user_message())),
                        Span::raw(ln.to_string()),
                    ]));
                }
                prev_speaker = Some("user".into());
            }
            DialogueLine::Agent { name, text } => {
                let speaker = format!("agent:{name}");
                let same_speaker = prev_speaker.as_deref() == Some(speaker.as_str());
                let name_tag = format!("● {name} ");
                let indent_width = UnicodeWidthStr::width(name_tag.as_str());
                let indent = " ".repeat(indent_width);
                for (i, ln) in text.lines().enumerate() {
                    if i == 0 && !same_speaker {
                        out.push(Line::from(vec![
                            Span::styled(
                                "● ",
                                Style::default()
                                    .fg(th.agent_message())
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("{name} "), Style::default().fg(th.muted())),
                            Span::raw(ln.to_string()),
                        ]));
                    } else {
                        out.push(Line::from(vec![
                            Span::raw(indent.clone()),
                            Span::raw(ln.to_string()),
                        ]));
                    }
                }
                prev_speaker = Some(speaker);
            }
            DialogueLine::System(t) => {
                for ln in t.lines() {
                    out.push(Line::from(vec![
                        Span::styled("· ", Style::default().fg(th.muted())),
                        Span::styled(
                            ln.to_string(),
                            Style::default()
                                .fg(th.warn())
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
                prev_speaker = Some("system".into());
            }
        }
    }
    out
}

/// 事件叙事化——把 raw `kind_tag` 翻译成人话图标 + 中文短语。
///
/// 返回 `(icon, color, narrative_text)`。调用方拼 `time  icon  who  narrative`。
///
/// 未知 kind 回退用 `row.summary` 原文，至少不会丢信息。设计参考
/// `docs/research/tui-v2-aesthetics.md` §4 Claude Code transcript 翻译表。
fn narrate_event(r: &EventRow) -> (&'static str, Color, String) {
    let t = theme();
    let summary = r.summary.as_str();
    match r.kind_tag {
        "platform_started" => ("★", t.success(), "平台启动".to_string()),
        "platform_stopping" => ("☾", t.muted(), "平台关闭".to_string()),
        "agent_spawning" => ("·", t.muted(), "招募中…".to_string()),
        "agent_ready" => ("●", t.success(), format!("上线 · {summary}")),
        "agent_shutting_down" => ("·", t.muted(), "准备下线".to_string()),
        "agent_dead" => ("✗", t.error(), format!("下线 · {summary}")),
        "task_created" => ("◇", t.user_message(), format!("新任务 · {summary}")),
        "task_dispatched" => ("→", t.user_message(), format!("接单 · {summary}")),
        "task_state_changed" => ("↻", t.warn(), summary.to_string()),
        "task_delivered" => ("✓", t.success(), format!("完成 · {summary}")),
        "task_blocked" => ("!", t.warn(), format!("阻塞 · {summary}")),
        "task_resumed" => ("▶", t.success(), format!("续上 · {summary}")),
        "task_cancelled" => ("×", t.muted(), format!("取消 · {summary}")),
        "user_prompted" => ("▍", t.user_message(), format!("用户 · {summary}")),
        "agent_responded" => ("●", t.agent_message(), format!("回话 · {summary}")),
        "user_intervention_sent" => ("✋", t.warn(), format!("指令 · {summary}")),
        "agent_interrupted" => ("⎋", t.warn(), format!("打断 · {summary}")),
        "task_intervention_applied" => ("✎", t.warn(), format!("追加 · {summary}")),
        "orchestrator_cc_received" => ("📣", t.info(), format!("抄送 · {summary}")),
        "conversation_handoff_requested" => ("⇄", t.agent_message(), format!("让贤 · {summary}")),
        "conversation_transferred" => ("⇄", t.agent_message(), format!("已让贤 · {summary}")),
        "conversation_returned" => ("↩", t.agent_message(), format!("回席 · {summary}")),
        "trigger_registered" => ("⏰", t.info(), format!("更漏登记 · {summary}")),
        "trigger_fired" => ("⏰", t.info(), format!("更漏响 · {summary}")),
        "trigger_dispatched" => ("→", t.info(), format!("更漏派活 · {summary}")),
        "trigger_skipped" => ("·", t.muted(), format!("更漏跳过 · {summary}")),
        "trigger_failed" => ("!", t.error(), format!("更漏失败 · {summary}")),
        "tool_call_started" => ("◈", t.tool_call(), format!("工具 · {summary}")),
        "tool_call_finished" => ("✓", t.muted(), format!("工具完 · {summary}")),
        "message_sent" => ("→", t.info(), format!("消息 · {summary}")),
        "message_received" => ("←", t.info(), format!("收信 · {summary}")),
        "skill_staged" | "skill_approved" | "skill_rejected" | "skill_activated" => {
            ("◆", t.agent_message(), format!("点将台 · {summary}"))
        }
        "no_role_matched" => ("?", t.warn(), format!("无匹配 · {summary}")),
        _ => ("·", r.color, summary.to_string()),
    }
}

fn is_noise_event(kind_tag: &str, summary: &str) -> bool {
    // Custom { label: "cc_*"/"rate_limit" } 默认吸掉——信息密度太低
    if kind_tag == "custom" {
        let s = summary.to_lowercase();
        return s.contains("cc_system_")
            || s.contains("cc_thinking_delta")
            || s.contains("rate_limit")
            || s.contains("cc_raw");
    }
    // thinking 细节不进事件流，玄女标题栏已显示「思考中」
    matches!(
        kind_tag,
        "thinking_started" | "thinking_finished" | "user_prompted" | "agent_responded"
    )
}

fn tool_arg_preview(tool: &str, args: &serde_json::Value) -> String {
    let cmd = args
        .get("command")
        .or_else(|| args.get("cmd"))
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if cmd.is_empty() {
        tool.to_string()
    } else {
        let clip: String = cmd.chars().take(40).collect();
        format!("{tool}={clip}")
    }
}

fn task_state_to_shelf(state: TaskState, thinking: bool) -> ShelfStatus {
    if thinking {
        return ShelfStatus::Busy;
    }
    match state {
        TaskState::New | TaskState::Ready | TaskState::InProgress | TaskState::Delivering => {
            ShelfStatus::Busy
        }
        TaskState::AwaitingInput | TaskState::Blocked => ShelfStatus::Idle,
        TaskState::Done | TaskState::Cancelled => ShelfStatus::Idle,
    }
}

fn status_marker(s: ShelfStatus) -> &'static str {
    match s {
        ShelfStatus::Idle => "🟢",
        ShelfStatus::Busy => "🔵",
        ShelfStatus::Dead => "⚫",
    }
}

/// 按字符数截断 / 右对齐。ASCII 场景 fallback——真正 CJK 场景用 truncate_by_width。
fn short_str(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count >= max {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    } else {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(max - count));
        out
    }
}

/// 按显示宽度（unicode-width）截断——CJK aware。
fn truncate_by_width(s: &str, max_width: usize) -> String {
    let total = UnicodeWidthStr::width(s);
    if total <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max_width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn short_id_of(id: AgentId) -> String {
    let s = id.to_string();
    match s.rsplit_once('-') {
        Some((_, uuid)) => uuid.chars().take(8).collect(),
        None => s.chars().take(8).collect(),
    }
}

fn humanize_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

async fn drive_tui(bus: EventBus, fuxi: Arc<Fuxi>, xuannv_id: AgentId) -> Result<()> {
    if let Err(e) = redirect_stderr_to_log("/tmp/fuxi.log") {
        eprintln!("⚠ 无法重定向 stderr 到日志文件: {e}。TUI 可能被日志污染");
    }

    install_panic_hook();

    enable_raw_mode().context("enable_raw_mode")?;
    let mut stdout = io::stdout();
    // bracketed paste：让 IME / 剪贴板整块内容一次进入，不被 KEY_POLL 拆分成逐键序列
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    // Kitty keyboard protocol：让现代终端（iTerm2 ≥ 3.5 / Ghostty / Kitty / Alacritty）
    // 把 Shift+Enter 真的送成 `Enter + SHIFT`。不支持的终端（macOS Terminal.app、
    // tmux 未开 passthrough 等）会返回错误——我们吞掉，由 Alt+Enter / Ctrl+J 兜底。
    let kitty_on = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = ReplApp::new(xuannv_id);
    let mut stream = bus.subscribe();

    let shelf = fuxi.clone_shelf();

    let loop_res: Result<()> = async {
        loop {
            sync_worker_state(&mut app, &fuxi.list_workers().await, &shelf).await;
            app.tick(Instant::now());

            terminal.draw(|f| app.draw(f))?;
            if app.should_quit {
                return Ok(());
            }

            tokio::select! {
                maybe_ev = stream.next() => match maybe_ev {
                    Some(Ok(ev)) => app.ingest(&ev),
                    Some(Err(e)) => tracing::warn!(error = %e, "bus 事件错误"),
                    None => return Ok(()),
                },
                maybe_term = tokio::task::spawn_blocking(|| {
                    if event::poll(KEY_POLL).unwrap_or(false) {
                        event::read().ok()
                    } else { None }
                }) => {
                    let Ok(Some(term_ev)) = maybe_term else { continue };
                    match term_ev {
                        TermEvent::Key(k) => {
                            if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                                continue;
                            }
                            match app.handle_key(k.code, k.modifiers) {
                                Some(Submit::Xuannv(text)) => {
                                    let _ = bus.publish(Event {
                                        meta: { let mut m = EventMeta::now(); m.agent = Some(xuannv_id); m },
                                        kind: EventKind::UserPrompted { text: text.clone() },
                                    });
                                    let fuxi_cl = fuxi.clone();
                                    let task = Task::new("user-turn", &text);
                                    tokio::spawn(async move {
                                        if let Err(e) = fuxi_cl.dispatch(xuannv_id, task).await {
                                            tracing::warn!(error = %e, "xuannv dispatch 失败");
                                        }
                                    });
                                }
                                Some(Submit::Worker(id, text)) => {
                                    let fuxi_cl = fuxi.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = fuxi_cl.intervene(id, false, &text).await {
                                            tracing::warn!(error = %e, "worker intervene 失败");
                                        }
                                    });
                                }
                                None => {}
                            }
                        }
                        TermEvent::Paste(s) => {
                            app.handle_paste(&s);
                        }
                        TermEvent::Mouse(m) => {
                            app.handle_mouse(m);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    .await;

    let _ = disable_raw_mode();
    if kitty_on {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    loop_res
}

/// 用 shelf 当只读源刷新 app 里的 worker 状态（worktree 不在事件流里）。
async fn sync_worker_state(
    app: &mut ReplApp,
    cards: &[AgentCard],
    shelf: &Arc<fuxi_orchestrator::Shelf>,
) {
    for card in cards {
        let status = shelf.status_of(card.id).await.unwrap_or(ShelfStatus::Dead);
        if card.id == app.xuannv_id {
            app.xuannv_status = status;
        } else if !app.tasks.iter().any(|t| t.worker == card.id) {
            app.upsert_idle(card.id, card.profile.role.clone(), status);
        } else if let Some(r) = app.idle_workers.iter_mut().find(|r| r.id == card.id) {
            r.status = status;
        }
        // worktree 挂到对应 task
        let worktree = shelf.worktree_of(card.id).await;
        for t in app.tasks.iter_mut().filter(|t| t.worker == card.id) {
            t.worktree = worktree.clone();
        }
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        prev(info);
    }));
}

#[cfg(unix)]
fn redirect_stderr_to_log(path: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)?;
    // SAFETY: dup2 对 valid fd 是安全调用；file 在作用域内有效。dup2 之后 fd 2
    // 独立引用 file 的底层 inode，file 被 drop 不影响 fd 2。
    let ret = unsafe { dup2(file.as_raw_fd(), 2) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn redirect_stderr_to_log(_path: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("stderr redirect only on unix"))
}

#[cfg(unix)]
unsafe extern "C" {
    fn dup2(oldfd: i32, newfd: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn mk_ev(agent: Option<AgentId>, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = agent;
        Event { meta, kind }
    }

    fn mk_row(kind_tag: &'static str, summary: &str) -> EventRow {
        EventRow {
            time: "12:00:00".into(),
            who: "test".into(),
            kind_tag,
            summary: summary.into(),
            color: Color::Reset,
            ingested_at: Instant::now(),
        }
    }

    #[test]
    fn narrate_event_translates_agent_lifecycle() {
        let ready = mk_row("agent_ready", "endpoint=session:abc");
        let (icon, _, text) = narrate_event(&ready);
        assert_eq!(icon, "●");
        assert!(text.contains("上线"), "实际={text:?}");

        let dead = mk_row("agent_dead", "EOF");
        let (icon, _, text) = narrate_event(&dead);
        assert_eq!(icon, "✗");
        assert!(text.contains("下线"));
    }

    #[test]
    fn narrate_event_translates_task_lifecycle() {
        let created = mk_row("task_created", "scout");
        assert!(narrate_event(&created).2.contains("新任务"));

        let dispatched = mk_row("task_dispatched", "luban → scout");
        assert!(narrate_event(&dispatched).2.contains("接单"));

        let delivered = mk_row("task_delivered", "scout");
        let (icon, _, _) = narrate_event(&delivered);
        assert_eq!(icon, "✓");

        let blocked = mk_row("task_blocked", "等待用户确认");
        let (icon, _, text) = narrate_event(&blocked);
        assert_eq!(icon, "!");
        assert!(text.contains("阻塞"));
    }

    #[test]
    fn narrate_event_unknown_kind_falls_back_to_summary() {
        let raw = mk_row("some_future_kind_i_dont_know", "payload stuff");
        let (_, _, text) = narrate_event(&raw);
        assert_eq!(text, "payload stuff", "未知 kind 至少保真 summary");
    }

    #[test]
    fn narrate_event_covers_handoff_and_cc() {
        assert!(
            narrate_event(&mk_row("conversation_handoff_requested", "a→b"))
                .2
                .contains("让贤")
        );
        assert!(
            narrate_event(&mk_row("orchestrator_cc_received", "luban"))
                .2
                .contains("抄送")
        );
    }

    // ───────── 鼠标交互：C4 click_registry 集成 ─────────

    fn mk_mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    #[test]
    fn mouse_left_click_on_roster_region_focuses_roster() {
        let mut app = ReplApp::stub();
        app.focus = Focus::Input;
        app.click
            .register(Rect::new(0, 0, 28, 40), ClickAction::FocusRoster);
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 5, 5));
        assert_eq!(app.focus, Focus::Roster);
    }

    #[test]
    fn mouse_left_click_on_input_region_focuses_input() {
        let mut app = ReplApp::stub();
        app.focus = Focus::Roster;
        app.click
            .register(Rect::new(28, 40, 60, 5), ClickAction::FocusInput);
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 40, 42));
        assert_eq!(app.focus, Focus::Input);
    }

    #[test]
    fn mouse_click_outside_any_region_does_nothing() {
        let mut app = ReplApp::stub();
        let before = app.focus;
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 10, 10));
        assert_eq!(app.focus, before, "空 registry 不应改 focus");
    }

    #[test]
    fn mouse_scroll_up_breaks_autoscroll_same_as_pgup() {
        let mut app = ReplApp::stub();
        app.last_dialogue_total = 100;
        app.last_dialogue_view = 10;
        app.dialogue_scroll = 90;
        assert!(app.dialogue_auto_scroll);
        app.handle_mouse(mk_mouse(MouseEventKind::ScrollUp, 0, 0));
        assert!(!app.dialogue_auto_scroll, "滚轮上应解除 auto-scroll");
        assert!(app.dialogue_scroll < 90, "应真往上翻一页");
    }

    #[test]
    fn mouse_right_button_is_ignored_in_v1() {
        let mut app = ReplApp::stub();
        let before = app.focus;
        app.click
            .register(Rect::new(0, 0, 28, 40), ClickAction::FocusRoster);
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Right), 5, 5));
        assert_eq!(app.focus, before, "v1 不处理右键点击");
    }

    fn mk_task_ev(agent: Option<AgentId>, task: TaskId, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = agent;
        meta.task = Some(task);
        Event { meta, kind }
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        let mut s = String::new();
        for x in 0..area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.trim_end().to_string()
    }

    // ───────── 基础输入：tui-textarea 接管后的行为 ─────────

    #[test]
    fn typing_and_enter_submits_to_xuannv() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, Some(Submit::Xuannv("hi".into())));
        assert!(app.input_text().is_empty());
    }

    #[test]
    fn empty_enter_returns_none() {
        let mut app = ReplApp::stub();
        assert!(
            app.handle_key(KeyCode::Enter, KeyModifiers::empty())
                .is_none()
        );
    }

    #[test]
    fn backspace_deletes_last_char() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty());
        app.handle_key(KeyCode::Backspace, KeyModifiers::empty());
        assert_eq!(app.input_text(), "a");
    }

    #[test]
    fn ctrl_c_requires_double_press_to_quit() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.should_quit);
        assert!(app.confirm_quit);
        app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
        assert!(!app.confirm_quit);
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit);
    }

    #[test]
    fn control_chars_in_input_are_ignored() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('\0'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('\t'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(app.input_text(), "a");
    }

    // ───────── 任务树：核心 Fix-D 断言 ─────────

    /// `TaskDispatched` 事件把门客从 idle 搬进任务列表。
    #[test]
    fn task_dispatched_event_appends_task_node() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let w = AgentId::new();
        app.ingest(&mk_ev(
            Some(w),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        assert!(app.idle_workers.iter().any(|r| r.id == w));

        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        assert_eq!(app.tasks.len(), 1, "应有一个 task 节点");
        assert_eq!(app.tasks[0].worker, w);
        assert_eq!(app.tasks[0].worker_role, "dev");
        assert!(
            !app.idle_workers.iter().any(|r| r.id == w),
            "门客应从空闲桶移走"
        );
    }

    /// Done 后延迟 prune；tick 前还在，tick 后没了。
    #[test]
    fn task_done_prunes_after_delay() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.prune_delay = Duration::from_millis(5);
        let w = AgentId::new();
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        assert_eq!(app.tasks.len(), 1);
        app.ingest(&mk_task_ev(
            None,
            tid,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        ));
        assert!(app.tasks[0].prune_after.is_some(), "Done 应触发 prune 定时");
        // 还没到期
        app.tick(Instant::now());
        assert_eq!(app.tasks.len(), 1, "未到期不能 prune");
        // 到期
        app.tick(Instant::now() + Duration::from_millis(50));
        assert!(app.tasks.is_empty(), "到期应清除 task");
    }

    /// AgentSpawning 的门客进入空闲桶。
    #[test]
    fn idle_worker_shows_in_idle_bucket() {
        let mut app = ReplApp::stub();
        let w = AgentId::new();
        app.ingest(&mk_ev(
            Some(w),
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ));
        assert_eq!(app.idle_workers.len(), 1);
        assert_eq!(app.idle_workers[0].role, "luban");
    }

    /// Tab 循环：玄女 → 每个 task 的 worker → 每个 idle → 玄女。
    #[test]
    fn tab_cycles_xuannv_tasks_idle_order() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let a = AgentId::new();
        let b = AgentId::new();
        // 两个门客都先 idle
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_ev(
            Some(b),
            EventKind::AgentSpawning {
                role: "pm".into(),
                cli: "cc".into(),
            },
        ));
        // 派一个 task 给 a
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: a },
        ));

        assert_eq!(app.active, ActiveTarget::Xuannv);
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(
            app.active,
            ActiveTarget::Worker(a),
            "Tab 1 应到 task 里的 a"
        );
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(
            app.active,
            ActiveTarget::Worker(b),
            "Tab 2 应到 idle 里的 b"
        );
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Xuannv, "Tab 3 回玄女");
    }

    #[test]
    fn esc_returns_to_xuannv() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        app.active = ActiveTarget::Worker(a);
        app.focus = Focus::Roster;

        app.handle_key(KeyCode::Esc, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Xuannv);
        assert_eq!(app.focus, Focus::Input);
    }

    // ───────── tui-textarea 多行 + 粘贴 ─────────

    #[test]
    fn tui_textarea_enter_submits_shift_enter_newlines() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT);
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty());
        assert_eq!(app.input_text(), "a\nb");
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, Some(Submit::Xuannv("a\nb".into())));
        assert!(app.input_text().is_empty());
    }

    /// 终端不送 Shift+Enter 时的兜底：Alt+Enter 作为换行同义。
    /// 理由见 `docs/research/tui-v2-aesthetics.md` §6——大多数终端默认
    /// 把 Shift+Enter 发成裸 Enter 不带 modifier，Alt+Enter 跨终端兼容性最好。
    #[test]
    fn alt_enter_also_newlines() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::ALT);
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty());
        assert_eq!(app.input_text(), "a\nb");
    }

    /// Ctrl+J 物理上 = `\n`，最古老最稳的换行兜底，任何终端都认。
    #[test]
    fn ctrl_j_also_newlines() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('j'), KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty());
        assert_eq!(app.input_text(), "a\nb");
    }

    #[test]
    fn bracketed_paste_fills_input() {
        let mut app = ReplApp::stub();
        app.handle_paste("hello\nworld");
        assert_eq!(app.input_text(), "hello\nworld");
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, Some(Submit::Xuannv("hello\nworld".into())));
    }

    // ───────── 滚动 ─────────

    #[test]
    fn scroll_up_breaks_auto_scroll_then_end_resumes() {
        let mut app = ReplApp::stub();
        app.last_dialogue_total = 100;
        app.last_dialogue_view = 10;
        app.dialogue_scroll = 90;
        assert!(app.dialogue_auto_scroll);
        app.handle_key(KeyCode::PageUp, KeyModifiers::empty());
        assert!(!app.dialogue_auto_scroll, "PgUp 应该冻结 auto_scroll");
        assert!(app.dialogue_scroll < 90);
        app.handle_key(KeyCode::End, KeyModifiers::empty());
        assert!(app.dialogue_auto_scroll, "End 应该回 auto_scroll");
    }

    // ───────── 多行折叠 ─────────

    #[test]
    fn consecutive_same_speaker_collapses_prefix() {
        let lines = [
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "line1".into(),
            },
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "line2".into(),
            },
        ];
        let rendered = render_dialogue_collapsed(lines.iter());
        assert_eq!(rendered.len(), 2, "应生成两行");
        let first = line_to_plain(&rendered[0]);
        let second = line_to_plain(&rendered[1]);
        assert!(
            first.starts_with("● 玄女 "),
            "第一行应带名字前缀: {first:?}"
        );
        assert!(!second.starts_with("● "), "第二行应去前缀缩进: {second:?}");
        assert!(second.contains("line2"));
    }

    /// 换 speaker 时名字前缀应重新出现。
    #[test]
    fn different_speakers_keep_prefix() {
        let lines = [
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "A".into(),
            },
            DialogueLine::Agent {
                name: "鲁班".into(),
                text: "B".into(),
            },
        ];
        let rendered = render_dialogue_collapsed(lines.iter());
        assert!(line_to_plain(&rendered[0]).starts_with("● 玄女 "));
        assert!(line_to_plain(&rendered[1]).starts_with("● 鲁班 "));
    }

    fn line_to_plain(line: &Line) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<&str>>()
            .join("")
    }

    // ───────── 对话桶路由 ─────────

    #[test]
    fn dialogue_buckets_split_by_speaker() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let worker = AgentId::new();
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));

        app.ingest(&mk_ev(
            Some(xid),
            EventKind::AgentResponded {
                text: "好的".into(),
            },
        ));
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::AgentResponded {
                text: "done".into(),
            },
        ));

        let x_bucket = app
            .dialogues
            .get(&ActiveTarget::Xuannv)
            .cloned()
            .unwrap_or_default();
        let w_bucket = app
            .dialogues
            .get(&ActiveTarget::Worker(worker))
            .cloned()
            .unwrap_or_default();
        assert_eq!(x_bucket.len(), 1);
        assert_eq!(w_bucket.len(), 1);
    }

    #[test]
    fn user_intervention_event_routes_to_worker_bucket() {
        let mut app = ReplApp::stub();
        let worker = AgentId::new();
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));

        app.ingest(&mk_ev(
            None,
            EventKind::UserInterventionSent {
                target: worker,
                mode: "append".into(),
                text: "加个单测".into(),
            },
        ));

        let w_bucket = app
            .dialogues
            .get(&ActiveTarget::Worker(worker))
            .cloned()
            .unwrap_or_default();
        assert_eq!(w_bucket.len(), 1);
        assert!(matches!(w_bucket[0], DialogueLine::User(_)));
    }

    /// AgentDead 事件 → 该 worker 的任务进 prune 队列，idle 桶移除。
    #[test]
    fn agent_dead_event_marks_tasks_for_prune() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.prune_delay = Duration::from_millis(5);
        let w = AgentId::new();
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        app.ingest(&mk_ev(
            Some(w),
            EventKind::AgentDead {
                cause: "ws closed".into(),
            },
        ));
        assert!(app.tasks[0].prune_after.is_some());
        // tick 后 task 应清理
        app.tick(Instant::now() + Duration::from_millis(50));
        assert!(app.tasks.is_empty());
    }

    #[test]
    fn handoff_event_switches_active_target() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let pm = AgentId::new();
        app.ingest(&mk_ev(
            Some(pm),
            EventKind::AgentSpawning {
                role: "pm".into(),
                cli: "cc".into(),
            },
        ));

        assert_eq!(app.active, ActiveTarget::Xuannv);
        app.ingest(&mk_ev(
            Some(xid),
            EventKind::ConversationHandoffRequested {
                from: xid,
                to: pm,
                reason: "澄清需求".into(),
                brief: None,
            },
        ));
        assert_eq!(app.active, ActiveTarget::Worker(pm));
        assert_eq!(app.focus, Focus::Input);
    }

    // ───────── 三栏 snapshot ─────────

    #[test]
    fn three_pane_snapshot_contains_expected_widgets() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let worker = AgentId::new();
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        app.push_line(
            ActiveTarget::Xuannv,
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "欢迎".into(),
            },
        );

        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();

        let all: String = (0..buf.area.height)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        // TestBackend 把 CJK 按 2 列存储，第二列是空格占位——比对前去空白
        let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("任务"), "左栏应标题 任务:\n{all}");
        assert!(compact.contains("玄女"), "玄女 字样缺失:\n{all}");
        assert!(compact.contains("欢迎"), "中栏对话内容缺失:\n{all}");
        assert!(compact.contains("空闲门客"), "idle header 应出现:\n{all}");
        assert!(all.contains("dev"), "门客 role 应出现:\n{all}");
    }

    /// roster focus 下方向键 + Enter 切 active。
    #[test]
    fn roster_up_down_then_enter_switches_active() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let a = AgentId::new();
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        app.focus = Focus::Roster;
        app.roster_state.select(Some(0)); // 玄女

        app.handle_key(KeyCode::Down, KeyModifiers::empty()); // 跳 IdleHeader → a
        app.handle_key(KeyCode::Down, KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a));
        assert_eq!(app.focus, Focus::Input);
    }

    #[test]
    fn f2_toggles_events_visible() {
        let mut app = ReplApp::stub();
        assert!(!app.events_visible);
        app.handle_key(KeyCode::F(2), KeyModifiers::empty());
        assert!(app.events_visible);
        app.handle_key(KeyCode::F(2), KeyModifiers::empty());
        assert!(!app.events_visible);
    }

    #[test]
    fn dialogue_cap_evicts_oldest() {
        let mut app = ReplApp::stub();
        for i in 0..(DIALOGUE_CAP + 10) {
            app.push_line(
                ActiveTarget::Xuannv,
                DialogueLine::System(format!("line-{i}")),
            );
        }
        let bucket = app.dialogues.get(&ActiveTarget::Xuannv).unwrap();
        assert_eq!(bucket.len(), DIALOGUE_CAP);
    }

    /// Thinking 事件：玄女触发 xuannv_thinking；门客触发 task.thinking。
    #[test]
    fn thinking_events_toggle_flags() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.ingest(&mk_ev(Some(xid), EventKind::ThinkingStarted));
        assert!(app.xuannv_thinking);
        app.ingest(&mk_ev(Some(xid), EventKind::ThinkingFinished));
        assert!(!app.xuannv_thinking);

        // 门客
        let w = AgentId::new();
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        app.ingest(&mk_ev(Some(w), EventKind::ThinkingStarted));
        assert!(app.tasks[0].thinking);
    }

    /// CJK 宽度：truncate_by_width 按 displayed width 截断，不按 chars。
    #[test]
    fn truncate_by_width_handles_cjk() {
        assert_eq!(truncate_by_width("abcdef", 5), "abcd…");
        // "伏羲" width=4；max_width=4 不截
        assert_eq!(truncate_by_width("伏羲", 4), "伏羲");
        // max_width=3 → 只够放 "伏"(2)+"…"
        assert_eq!(truncate_by_width("伏羲", 3), "伏…");
    }

    /// 事件面板噪声过滤：cc_system_ / thinking 不进面板。
    #[test]
    fn noise_filter_hides_low_value_events() {
        assert!(is_noise_event("thinking_started", ""));
        assert!(is_noise_event("user_prompted", ""));
        assert!(is_noise_event("custom", "cc_system_start"));
        assert!(!is_noise_event("tool_call_started", "tool=Bash"));
        assert!(!is_noise_event("task_dispatched", "to=abc"));
    }

    // ───────── PATH 探测 ─────────

    #[test]
    fn require_fuxi_in_path_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path_env = dir.path().as_os_str();
        let res = require_fuxi_in_path("fuxi", Some(path_env));
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("scripts/install.sh"),
            "error 必须指向 scripts/install.sh；实际：{msg}"
        );
    }

    #[test]
    fn require_fuxi_in_path_errors_when_path_env_unset() {
        let res = require_fuxi_in_path("fuxi", None);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("scripts/install.sh"));
    }

    #[cfg(unix)]
    #[test]
    fn require_fuxi_in_path_finds_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("fuxi");
        std::fs::write(&bin_path, "#!/bin/sh\necho ok\n").unwrap();
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let res = require_fuxi_in_path("fuxi", Some(dir.path().as_os_str()));
        assert!(res.is_ok(), "应找到 binary；实际：{res:?}");
    }

    #[cfg(unix)]
    #[test]
    fn require_fuxi_in_path_skips_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("fuxi");
        std::fs::write(&bin_path, b"not exe").unwrap();
        let res = require_fuxi_in_path("fuxi", Some(dir.path().as_os_str()));
        assert!(res.is_err());
    }
}
