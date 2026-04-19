//! `fuxi`（无参）—— v1 用户唯一入口：三栏 REPL TUI。
//!
//! 设计锚：
//! - `docs/architecture-v1.md` §M1.4（三栏 TUI）+ §M1.5（让贤订阅）
//! - `docs/research/tui-3pane-design.md`（骨架 + Key routing）
//!
//! 布局 `Layout::horizontal([Length(26), Min(40), Length(28)])`：
//!   - **左栏**：roster（门客清单），F2 折叠/展开底部事件流
//!   - **中栏**：vertical([Min(5), Length(3), Length(1)]) = 对话 + 输入 + 状态
//!   - **右栏**：元信息（当前 active 目标的 task/worktree/status）
//!
//! `active: ActiveTarget` 决定用户输入送给谁：
//!   - `Xuannv` → `Fuxi::dispatch(xuannv_id, new Task("user-turn", text))` 新起 turn
//!   - `Worker(id)` → `Fuxi::intervene(id, false, text)` 追加式介入（玄女抄送）
//!
//! Tab 循环切 active；Esc 速回玄女；F2 toggle 事件流；Ctrl-C 两段式退出。

use crate::daemon::Daemon;
use crate::ipc;
use anyhow::{Context, Result, anyhow};
use clap::Args as ClapArgs;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
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
use fuxi_events::EventBus;
use fuxi_firehose::{FirehoseApp, Hub};
use fuxi_orchestrator::{Fuxi, FuxiConfig, ShelfStatus, WorkerKind};
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

/// 每个对话桶最多保留多少行。
const DIALOGUE_CAP: usize = 500;
/// 每秒刷 UI 的键盘 poll 窗口。
const KEY_POLL: Duration = Duration::from_millis(50);

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
        // Unix 下要可执行才算数；非 Unix 见到文件就当数。
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
/// `command not found`，整个 platform 失语。失败时 error message 必须明确指向
/// `scripts/install.sh`，不许让用户自己猜要装什么。
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
    // 入装预检——玄女的 Bash 工具会调 `fuxi spawn/dispatch ...`，shell 找不到 fuxi
    // 就全盘瘫痪。所以在 TUI 起来之前先 fail-fast，让用户先跑 install.sh。
    require_fuxi_in_path("fuxi", std::env::var_os("PATH").as_deref())?;

    // skills 预检——找不到直接报错提示，不要进 TUI 再卡死
    if skill_loader::skills_root().is_none() {
        return Err(anyhow!(
            "找不到 skills 目录：试过 $FUXI_SKILLS_DIR / git-root/skills / ./skills / $HOME/.fuxi/skills 都不在。\n\
             建议：export FUXI_SKILLS_DIR=/Users/e0_7/fuxi/skills  （或把 fuxi/skills 软链到 ~/.fuxi/skills）"
        ));
    }

    // 1. EventBus + Fuxi orchestrator
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

    // 2. Firehose Hub（给 `fuxi watch` 外部观察留口；REPL 自己不用它）
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

    // 3. daemon socket——玄女 Bash `fuxi dispatch/intervene ...` 走这里
    let sock_path = args.sock_path.clone().unwrap_or_else(ipc::socket_path);
    // SAFETY: daemon::serve 自己会 parent-dir ensure + 清残留；这里只把路径传进 env
    // 让 cc 子进程继承到 $FUXI_SOCK
    unsafe {
        std::env::set_var("FUXI_SOCK", &sock_path);
    }
    // REPL 场景的 scheduler：用内存库，退出即丢；足够测试 `fuxi cron fire` 等命令。
    let sched_store = TriggerStore::connect_memory()
        .await
        .context("创建 scheduler 内存库")?;
    let keeper = Arc::new(Keeper::new(
        sched_store.clone(),
        bus.clone(),
        Arc::new(SystemClock),
    ));
    let keeper_task = Arc::clone(&keeper).spawn();
    let daemon = Daemon::new(fuxi.clone(), bus.clone(), sched_store, keeper);
    let daemon_shutdown = daemon.shutdown_handle();
    let sock_for_task = sock_path.clone();
    let daemon_task = tokio::spawn(async move {
        if let Err(e) = daemon.serve(&sock_for_task).await {
            tracing::warn!(error = %e, "daemon serve 异常");
        }
    });

    // 4. 发 PlatformStarted 事件——REPL 启动点
    let _ = bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    });

    // 5. spawn 玄女——从 skills/<role>/SKILL.md 读 profile
    let loaded = skill_loader::load(&args.xuannv_role)
        .with_context(|| format!("加载 skills/{}/SKILL.md", args.xuannv_role))?;
    let xuannv_profile = loaded.profile.clone();
    let cc_cfg = CcLaunchConfig {
        append_system_prompt: if loaded.append_system_prompt.is_empty() {
            None
        } else {
            Some(loaded.append_system_prompt)
        },
        allowed_tools: loaded.allowed_tools,
        ..Default::default()
    };
    let xuannv_id = fuxi
        .spawn_worker(xuannv_profile, WorkerKind::Cc(cc_cfg))
        .await
        .context("玄女 spawn 失败")?;
    fuxi.set_xuannv(xuannv_id).await;
    tracing::info!(xuannv = %xuannv_id, "玄女已就绪");

    // 6. 发 greet task 让玄女主动开口（cc headless 没 prompt 不说话）
    let greet = Task::new(
        "greet",
        "用户刚启动 fuxi REPL。请用一句话（十字以内）主动问好，邀请用户提需求。不要自我介绍。",
    );
    if let Err(e) = fuxi.dispatch(xuannv_id, greet).await {
        tracing::warn!(error = %e, "greet dispatch 失败，继续");
    }

    // 7. 进入 TUI 主循环
    let outcome = drive_tui(bus, fuxi.clone(), xuannv_id).await;

    // 8. 收尾——无论 loop 怎么退都走 shutdown
    daemon_shutdown.notify_waiters();
    if let Err(e) = fuxi.shutdown().await {
        tracing::warn!(error = %e, "fuxi shutdown 部分失败");
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    hub_task.abort();
    daemon_task.abort();
    keeper_task.abort();

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

/// 左栏一行。
#[derive(Debug, Clone)]
pub(crate) struct RosterRow {
    pub id: AgentId,
    pub role: String,
    pub name: String,
    pub status: ShelfStatus,
}

/// 右栏展示的门客任务元信息。
#[derive(Debug, Clone)]
pub(crate) struct TaskMeta {
    pub title: String,
    pub state: TaskState,
    pub task_id: Option<TaskId>,
    pub started_at: Instant,
    pub worktree: Option<PathBuf>,
    pub thinking: bool,
}

impl Default for TaskMeta {
    fn default() -> Self {
        Self {
            title: String::new(),
            state: TaskState::New,
            task_id: None,
            started_at: Instant::now(),
            worktree: None,
            thinking: false,
        }
    }
}

/// 用户按 Enter 后 TUI 计算出的提交意图——由 drive_tui 翻译成对应的 Fuxi 调用。
/// 分 Xuannv / Worker 两种，区分 dispatch（新 task）和 intervene（追加介入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Submit {
    /// 发给玄女 = 新 task dispatch。
    Xuannv(String),
    /// 发给门客 = 追加式 intervene（mode=append）。
    Worker(AgentId, String),
}

/// REPL TUI 的核心状态。纯逻辑，不 own terminal——便于单测。
pub(crate) struct ReplApp {
    pub(crate) xuannv_id: AgentId,
    pub(crate) focus: Focus,
    pub(crate) active: ActiveTarget,
    pub(crate) dialogues: HashMap<ActiveTarget, VecDeque<DialogueLine>>,
    pub(crate) task_meta: HashMap<AgentId, TaskMeta>,
    pub(crate) roster: Vec<RosterRow>,
    pub(crate) roster_state: ListState,
    pub(crate) events_visible: bool,
    pub(crate) events: FirehoseApp,
    pub(crate) input: String,
    pub(crate) should_quit: bool,
    pub(crate) confirm_quit: bool,
}

impl ReplApp {
    pub(crate) fn new(xuannv_id: AgentId) -> Self {
        let mut app = Self {
            xuannv_id,
            focus: Focus::Input,
            active: ActiveTarget::Xuannv,
            dialogues: HashMap::new(),
            task_meta: HashMap::new(),
            roster: Vec::new(),
            roster_state: ListState::default(),
            events_visible: false,
            events: FirehoseApp::new(),
            input: String::new(),
            should_quit: false,
            confirm_quit: false,
        };
        // 玄女自身先放 roster 首位（status 先置 Idle；后续 spawn 事件会纠正）
        app.roster.push(RosterRow {
            id: xuannv_id,
            role: "xuannv".to_string(),
            name: "玄女".to_string(),
            status: ShelfStatus::Idle,
        });
        app.roster_state.select(Some(0));
        app
    }

    /// 构造仅含玄女的 app——测试帮手。
    #[cfg(test)]
    fn stub() -> Self {
        Self::new(AgentId::new())
    }

    pub(crate) fn push_line(&mut self, target: ActiveTarget, line: DialogueLine) {
        let bucket = self.dialogues.entry(target).or_default();
        if bucket.len() == DIALOGUE_CAP {
            bucket.pop_front();
        }
        bucket.push_back(line);
    }

    /// 把一条 Event 按"对话事件 vs 事件流"分路处理。
    /// - 事件流窗口总是吃
    /// - 对话 / 状态 相关的按 target 分桶入 dialogues / 更新 task_meta / 更新 roster
    pub(crate) fn ingest(&mut self, ev: &Event) {
        self.events.ingest(ev);
        let who = ev.meta.agent;
        let xuannv = self.xuannv_id;
        // 不用闭包——避免 borrow checker 在下方调用 `self.set_roster_status(...)`
        // 后再借用闭包时撞上 E0502。内联 match 两行即可。
        #[inline]
        fn target_for(xuannv: AgentId, id: AgentId) -> ActiveTarget {
            if id == xuannv {
                ActiveTarget::Xuannv
            } else {
                ActiveTarget::Worker(id)
            }
        }

        match &ev.kind {
            EventKind::AgentSpawning { role, .. } => {
                if let Some(id) = who {
                    self.upsert_roster(id, role.clone(), ShelfStatus::Idle);
                }
            }
            EventKind::AgentReady { .. } => {
                if let Some(id) = who {
                    self.set_roster_status(id, ShelfStatus::Idle);
                }
            }
            EventKind::AgentDead { cause } => {
                if let Some(id) = who {
                    self.set_roster_status(id, ShelfStatus::Dead);
                    self.push_line(
                        target_for(xuannv, id),
                        DialogueLine::System(format!("⚠ 下线：{cause}")),
                    );
                }
            }
            EventKind::UserPrompted { text } => {
                // 用户说的话挂到当前主对象
                self.push_line(self.active, DialogueLine::User(text.clone()));
            }
            EventKind::UserInterventionSent { target, text, .. } => {
                // 用户对某门客的追加介入——画在门客桶里
                self.push_line(
                    ActiveTarget::Worker(*target),
                    DialogueLine::User(text.clone()),
                );
            }
            EventKind::AgentResponded { text } => {
                if let Some(id) = who {
                    let name = self.agent_display_name(id);
                    self.push_line(
                        target_for(xuannv, id),
                        DialogueLine::Agent {
                            name,
                            text: text.clone(),
                        },
                    );
                }
            }
            EventKind::ThinkingStarted => {
                if let Some(id) = who {
                    self.task_meta.entry(id).or_default().thinking = true;
                }
            }
            EventKind::ThinkingFinished => {
                if let Some(id) = who {
                    self.task_meta.entry(id).or_default().thinking = false;
                }
            }
            EventKind::TaskDispatched { to } => {
                let m = self.task_meta.entry(*to).or_default();
                m.state = TaskState::InProgress;
                m.started_at = Instant::now();
                if let Some(tid) = ev.meta.task {
                    m.task_id = Some(tid);
                }
                self.set_roster_status(*to, ShelfStatus::Busy);
            }
            EventKind::TaskCreated { title, .. } => {
                if let Some(id) = who {
                    let m = self.task_meta.entry(id).or_default();
                    m.title = title.clone();
                    if let Some(tid) = ev.meta.task {
                        m.task_id = Some(tid);
                    }
                }
            }
            EventKind::TaskStateChanged { to, .. } => {
                if let Some(id) = who {
                    let m = self.task_meta.entry(id).or_default();
                    m.state = *to;
                }
            }
            EventKind::TaskDelivered { .. } | EventKind::TaskCancelled { .. } => {
                if let Some(id) = who {
                    self.set_roster_status(id, ShelfStatus::Idle);
                }
            }
            EventKind::ConversationHandoffRequested { to, .. } => {
                // 让贤：主对话权切到 to
                self.active = target_for(xuannv, *to);
                self.focus = Focus::Input;
                self.resync_roster_selection();
            }
            _ => {}
        }
    }

    /// 新增/更新 roster 条目——幂等。
    fn upsert_roster(&mut self, id: AgentId, role: String, status: ShelfStatus) {
        if let Some(row) = self.roster.iter_mut().find(|r| r.id == id) {
            row.role = role;
            row.status = status;
        } else {
            let name = role.clone();
            self.roster.push(RosterRow {
                id,
                role,
                name,
                status,
            });
        }
    }

    fn set_roster_status(&mut self, id: AgentId, status: ShelfStatus) {
        if let Some(row) = self.roster.iter_mut().find(|r| r.id == id) {
            row.status = status;
        }
    }

    fn agent_display_name(&self, id: AgentId) -> String {
        self.roster
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| short_id_of(id))
    }

    /// 让 roster_state.selected 与 active 保持一致。
    fn resync_roster_selection(&mut self) {
        let want = match self.active {
            ActiveTarget::Xuannv => self.roster.iter().position(|r| r.id == self.xuannv_id),
            ActiveTarget::Worker(id) => self.roster.iter().position(|r| r.id == id),
        };
        self.roster_state.select(want);
    }

    /// Tab 循环切 active：Xuannv → 门客1 → 门客2 → ... → Xuannv。
    pub(crate) fn cycle_active_to_next(&mut self) {
        if self.roster.is_empty() {
            return;
        }
        let cur_idx = match self.active {
            ActiveTarget::Xuannv => self.roster.iter().position(|r| r.id == self.xuannv_id),
            ActiveTarget::Worker(id) => self.roster.iter().position(|r| r.id == id),
        };
        let next_idx = match cur_idx {
            None => 0,
            Some(i) => (i + 1) % self.roster.len(),
        };
        let row = &self.roster[next_idx];
        self.active = if row.id == self.xuannv_id {
            ActiveTarget::Xuannv
        } else {
            ActiveTarget::Worker(row.id)
        };
        self.roster_state.select(Some(next_idx));
    }

    /// Esc：速回玄女。
    pub(crate) fn reset_to_xuannv(&mut self) {
        self.active = ActiveTarget::Xuannv;
        self.focus = Focus::Input;
        self.resync_roster_selection();
    }

    fn roster_up(&mut self) {
        if self.roster.is_empty() {
            return;
        }
        let cur = self.roster_state.selected().unwrap_or(0);
        let next = cur.saturating_sub(1);
        self.roster_state.select(Some(next));
    }

    fn roster_down(&mut self) {
        if self.roster.is_empty() {
            return;
        }
        let cur = self.roster_state.selected().unwrap_or(0);
        let next = (cur + 1).min(self.roster.len() - 1);
        self.roster_state.select(Some(next));
    }

    /// Enter on roster = 切 active 到高亮行 + 回到 Input focus。
    fn roster_enter(&mut self) {
        let Some(idx) = self.roster_state.selected() else {
            return;
        };
        let Some(row) = self.roster.get(idx) else {
            return;
        };
        self.active = if row.id == self.xuannv_id {
            ActiveTarget::Xuannv
        } else {
            ActiveTarget::Worker(row.id)
        };
        self.focus = Focus::Input;
    }

    /// 处理一次按键。返回 Some(Submit) 表示有待提交意图；否则 None。
    ///
    /// 双层：先过全局键（Ctrl-C / Tab / Esc / F2），剩下按 focus 分派。
    pub(crate) fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<Submit> {
        // Ctrl-C 两段式
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
        // 任何其它键重置确认
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
            _ => {}
        }

        // focus=Roster：↑/↓/Enter；其它可打印字符自动转 Input（忍让）
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
                    self.input.push(c);
                    return None;
                }
                _ => return None,
            }
        }

        // focus=Input
        match code {
            KeyCode::Enter => {
                let t = std::mem::take(&mut self.input);
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    return None;
                }
                match self.active {
                    ActiveTarget::Xuannv => Some(Submit::Xuannv(trimmed.to_string())),
                    ActiveTarget::Worker(id) => Some(Submit::Worker(id, trimmed.to_string())),
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Char(c) => {
                if !c.is_control() && c != '\t' {
                    self.input.push(c);
                }
                None
            }
            _ => None,
        }
    }

    pub(crate) fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        let root = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(26),
                Constraint::Min(40),
                Constraint::Length(28),
            ])
            .split(f.area());

        // 左栏：roster + 折叠事件流
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(6),
                Constraint::Length(if self.events_visible { 10 } else { 0 }),
            ])
            .split(root[0]);
        self.draw_roster(f, left[0]);
        if self.events_visible {
            self.draw_events(f, left[1]);
        }

        // 中栏：dialogue + input + status
        let center = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(root[1]);
        self.draw_dialogue(f, center[0]);
        self.draw_input(f, center[1]);
        self.draw_status(f, center[2]);

        self.draw_meta(f, root[2]);
    }

    fn draw_roster(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let items: Vec<ListItem> = self
            .roster
            .iter()
            .map(|row| {
                let marker = match row.status {
                    ShelfStatus::Idle => "🟢",
                    ShelfStatus::Busy => "🔵",
                    ShelfStatus::Dead => "⚫",
                };
                let active_mark = if self.is_active_row(row) {
                    "▶ "
                } else {
                    "  "
                };
                let style = if row.status == ShelfStatus::Dead {
                    Style::default().fg(Color::DarkGray)
                } else if row.id == self.xuannv_id {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(vec![
                    Span::raw(active_mark),
                    Span::styled(format!("{marker} "), Style::default()),
                    Span::styled(short_or_pad(&row.name, 8), style),
                    Span::raw(" "),
                    Span::styled(
                        short_or_pad(&row.role, 8),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if self.focus == Focus::Roster {
                " ▸ 门客 "
            } else {
                " 门客 "
            })
            .border_style(Style::default().fg(if self.focus == Focus::Roster {
                Color::Cyan
            } else {
                Color::DarkGray
            }));
        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut self.roster_state);
    }

    fn is_active_row(&self, row: &RosterRow) -> bool {
        match self.active {
            ActiveTarget::Xuannv => row.id == self.xuannv_id,
            ActiveTarget::Worker(id) => row.id == id,
        }
    }

    fn draw_events(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" events ")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let rows = self.events.visible_rows();
        let available = inner.height as usize;
        let start = rows.len().saturating_sub(available);
        let lines: Vec<Line> = rows[start..]
            .iter()
            .map(|r| {
                Line::from(vec![
                    Span::styled(r.time.clone(), Style::default().fg(Color::DarkGray)),
                    Span::raw(" "),
                    Span::styled(
                        short_or_pad(r.kind_tag, 18),
                        Style::default().fg(r.color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::raw(short_or_pad(
                        &r.summary,
                        inner.width.saturating_sub(28) as usize,
                    )),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn draw_dialogue(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let active_label = match self.active {
            ActiveTarget::Xuannv => "玄女".to_string(),
            ActiveTarget::Worker(id) => self.agent_display_name(id),
        };
        let thinking = match self.active {
            ActiveTarget::Xuannv => self
                .task_meta
                .get(&self.xuannv_id)
                .map(|m| m.thinking)
                .unwrap_or(false),
            ActiveTarget::Worker(id) => {
                self.task_meta.get(&id).map(|m| m.thinking).unwrap_or(false)
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
            .border_style(Style::default().fg(Color::Cyan));

        let empty = VecDeque::new();
        let bucket = self.dialogues.get(&self.active).unwrap_or(&empty);
        let lines: Vec<Line> = bucket.iter().flat_map(render_dialogue_lines).collect();
        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((dialogue_scroll_offset(bucket, area.height), 0));
        f.render_widget(para, area);
    }

    fn draw_input(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let prefix = match self.active {
            ActiveTarget::Xuannv => "玄女> ".to_string(),
            ActiveTarget::Worker(id) => format!("{}> ", self.agent_display_name(id)),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 你 ")
            .border_style(Style::default().fg(if self.focus == Focus::Input {
                Color::Green
            } else {
                Color::DarkGray
            }));
        let text = Line::from(vec![
            Span::styled(prefix, Style::default().fg(Color::DarkGray)),
            Span::raw(&self.input),
            Span::styled(
                "_",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ]);
        f.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_status(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let hint = " Tab 切门客 | Esc 回玄女 | F2 事件流 | Enter 发送 | Ctrl-C 退出 ";
        let para = Paragraph::new(hint).style(Style::default().fg(Color::Black).bg(Color::Gray));
        f.render_widget(para, area);
    }

    fn draw_meta(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let (lines, title) = match self.active {
            ActiveTarget::Xuannv => {
                let m = self.task_meta.get(&self.xuannv_id);
                let row = self.roster.iter().find(|r| r.id == self.xuannv_id);
                (
                    vec![
                        Line::from(format!("agent    {}", short_id_of(self.xuannv_id))),
                        Line::from(format!(
                            "status   {:?}",
                            row.map(|r| r.status).unwrap_or(ShelfStatus::Idle)
                        )),
                        Line::from(format!(
                            "task     {}",
                            m.map(|x| x.title.as_str()).unwrap_or("-")
                        )),
                        Line::from(format!(
                            "state    {:?}",
                            m.map(|x| x.state).unwrap_or(TaskState::New)
                        )),
                        Line::from(format!(
                            "elapsed  {}",
                            m.map(|x| humanize_elapsed(x.started_at.elapsed()))
                                .unwrap_or("-".into())
                        )),
                    ],
                    " 玄女 · 元信息 ",
                )
            }
            ActiveTarget::Worker(id) => {
                let m = self.task_meta.get(&id);
                let row = self.roster.iter().find(|r| r.id == id);
                (
                    vec![
                        Line::from(format!("agent    {}", short_id_of(id))),
                        Line::from(format!(
                            "role     {}",
                            row.map(|r| r.role.as_str()).unwrap_or("-")
                        )),
                        Line::from(format!(
                            "status   {:?}",
                            row.map(|r| r.status).unwrap_or(ShelfStatus::Dead)
                        )),
                        Line::from(format!(
                            "task     {}",
                            m.map(|x| x.title.as_str()).unwrap_or("-")
                        )),
                        Line::from(format!(
                            "state    {:?}",
                            m.map(|x| x.state).unwrap_or(TaskState::New)
                        )),
                        Line::from(format!(
                            "elapsed  {}",
                            m.map(|x| humanize_elapsed(x.started_at.elapsed()))
                                .unwrap_or("-".into())
                        )),
                        Line::from(format!(
                            "worktree {}",
                            m.and_then(|x| x.worktree.as_ref())
                                .map(|p| p.display().to_string())
                                .unwrap_or("-".into())
                        )),
                    ],
                    " 门客 · 元信息 ",
                )
            }
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

fn render_dialogue_lines(line: &DialogueLine) -> Vec<Line<'_>> {
    match line {
        DialogueLine::User(t) => t
            .lines()
            .map(|ln| {
                Line::from(vec![
                    Span::styled("你> ", Style::default().fg(Color::Green)),
                    Span::raw(ln.to_string()),
                ])
            })
            .collect(),
        DialogueLine::Agent { name, text } => text
            .lines()
            .map(|ln| {
                Line::from(vec![
                    Span::styled(
                        format!("{name}> "),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(ln.to_string()),
                ])
            })
            .collect(),
        DialogueLine::System(t) => vec![Line::from(Span::styled(
            t.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ))],
    }
}

fn dialogue_scroll_offset(bucket: &VecDeque<DialogueLine>, area_height: u16) -> u16 {
    let visible = area_height.saturating_sub(2) as usize;
    let total: usize = bucket.iter().map(|l| render_dialogue_lines(l).len()).sum();
    if total > visible {
        (total - visible) as u16
    } else {
        0
    }
}

fn short_or_pad(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count >= max {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    } else {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(max - count));
        out
    }
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
    // 把 stderr 重定向到文件，否则 tracing / claude inherited stderr / panic 会直接
    // 砸到 ratatui 的 alt-screen 上污染对话区。
    if let Err(e) = redirect_stderr_to_log("/tmp/fuxi.log") {
        eprintln!("⚠ 无法重定向 stderr 到日志文件: {e}。TUI 可能被日志污染");
    }

    install_panic_hook();

    enable_raw_mode().context("enable_raw_mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = ReplApp::new(xuannv_id);
    let mut stream = bus.subscribe();

    // 拿 shelf 只读 handle 用来拉 worktree + 初始 roster——worktree 数据不在事件里。
    let shelf = fuxi.clone_shelf();

    let loop_res: Result<()> = async {
        loop {
            // 每帧同步一次 roster 的 worktree/status（开销小）
            sync_roster_from_shelf(&mut app, &fuxi.list_workers().await, &shelf).await;

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
                maybe_key = tokio::task::spawn_blocking(|| {
                    if event::poll(KEY_POLL).unwrap_or(false) {
                        match event::read() {
                            Ok(TermEvent::Key(k)) => Some(k),
                            _ => None,
                        }
                    } else { None }
                }) => {
                    let Ok(Some(key)) = maybe_key else { continue };
                    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    match app.handle_key(key.code, key.modifiers) {
                        Some(Submit::Xuannv(text)) => {
                            // 先发 UserPrompted 让对话区立刻有用户行
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
            }
        }
    }
    .await;

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    loop_res
}

/// 从 shelf 拉 worktree + status 补齐 roster（事件流不带 worktree）。
async fn sync_roster_from_shelf(
    app: &mut ReplApp,
    cards: &[AgentCard],
    shelf: &Arc<fuxi_orchestrator::Shelf>,
) {
    for card in cards {
        let status = shelf.status_of(card.id).await.unwrap_or(ShelfStatus::Dead);
        app.upsert_roster(card.id, card.profile.role.clone(), status);
        let worktree = shelf.worktree_of(card.id).await;
        app.task_meta.entry(card.id).or_default().worktree = worktree;
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
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

    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        let mut s = String::new();
        for x in 0..area.width {
            s.push_str(buf[(x, y)].symbol());
        }
        s.trim_end().to_string()
    }

    // ── 基础输入/键盘（原有单测保留骨干，迁移到 Submit 语义） ──

    #[test]
    fn typing_and_enter_submits_to_xuannv() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, Some(Submit::Xuannv("hi".into())));
        assert!(app.input.is_empty());
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
        assert_eq!(app.input, "a");
    }

    #[test]
    fn ctrl_c_requires_double_press_to_quit() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.should_quit);
        assert!(app.confirm_quit);
        // 期间其它键重置确认
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
        assert_eq!(app.input, "a");
    }

    // ── 三栏 TUI 新测 ──

    /// Tab 循环切 active：玄女 → 门客1 → 门客2 → 玄女。
    #[test]
    fn tab_cycles_active_through_roster() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        let b = AgentId::new();
        app.upsert_roster(a, "dev".into(), ShelfStatus::Idle);
        app.upsert_roster(b, "pm".into(), ShelfStatus::Idle);

        assert_eq!(app.active, ActiveTarget::Xuannv);
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a));
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(b));
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Xuannv);
    }

    /// Esc 速回玄女。
    #[test]
    fn esc_returns_to_xuannv() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.upsert_roster(a, "dev".into(), ShelfStatus::Idle);
        app.active = ActiveTarget::Worker(a);
        app.focus = Focus::Roster;

        app.handle_key(KeyCode::Esc, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Xuannv);
        assert_eq!(app.focus, Focus::Input);
    }

    /// 对玄女输入 → Submit::Xuannv（dispatch 路径）。
    #[test]
    fn enter_on_xuannv_active_submits_xuannv() {
        let mut app = ReplApp::stub();
        for c in "好".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty());
        }
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, Some(Submit::Xuannv("好".into())));
    }

    /// 对门客输入 → Submit::Worker(id, text)（intervene 路径）。
    #[test]
    fn enter_on_worker_active_submits_worker() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.upsert_roster(a, "dev".into(), ShelfStatus::Idle);
        app.active = ActiveTarget::Worker(a);

        for c in "hi".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty());
        }
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, Some(Submit::Worker(a, "hi".into())));
    }

    /// 玄女说话进玄女桶；门客说话进门客桶。
    #[test]
    fn dialogue_buckets_split_by_speaker() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let worker = AgentId::new();
        app.upsert_roster(worker, "dev".into(), ShelfStatus::Idle);

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
        assert_eq!(x_bucket.len(), 1, "玄女桶应有一条");
        assert_eq!(w_bucket.len(), 1, "门客桶应有一条");
    }

    /// UserInterventionSent 应进到 target 门客的桶。
    #[test]
    fn user_intervention_event_routes_to_worker_bucket() {
        let mut app = ReplApp::stub();
        let worker = AgentId::new();
        app.upsert_roster(worker, "dev".into(), ShelfStatus::Idle);

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

    /// AgentDead 事件 → roster 条目置 Dead（UI 灰化）。
    #[test]
    fn agent_dead_event_marks_roster_dead() {
        let mut app = ReplApp::stub();
        let worker = AgentId::new();
        app.upsert_roster(worker, "dev".into(), ShelfStatus::Busy);

        app.ingest(&mk_ev(
            Some(worker),
            EventKind::AgentDead {
                cause: "ws closed".into(),
            },
        ));
        let row = app.roster.iter().find(|r| r.id == worker).expect("roster");
        assert_eq!(row.status, ShelfStatus::Dead);
    }

    /// 让贤事件（ConversationHandoffRequested）→ active 跟着切到 to。
    #[test]
    fn handoff_event_switches_active_target() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let pm = AgentId::new();
        app.upsert_roster(pm, "pm".into(), ShelfStatus::Idle);

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

    /// 三栏 TestBackend snapshot——左栏 roster / 中栏对话标题 / 右栏 meta 标题都在。
    #[test]
    fn three_pane_snapshot_contains_expected_widgets() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        // 喂一个门客 roster
        let worker = AgentId::new();
        app.upsert_roster(worker, "dev".into(), ShelfStatus::Idle);
        // 给玄女桶一条对话
        app.push_line(
            ActiveTarget::Xuannv,
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "欢迎".into(),
            },
        );

        let backend = TestBackend::new(120, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();

        let all: String = (0..buf.area.height)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        // Why 去空格比对：TestBackend 把 CJK 宽字符按 2 列存储（第二列是空格占位），
        // 所以 "门客" 在 buffer 里是 "门 客"；直接 contains 不可靠。
        let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("门客"), "左栏 roster 标题缺失:\n{all}");
        assert!(compact.contains("玄女"), "中/左栏 玄女 字样缺失:\n{all}");
        assert!(compact.contains("元信息"), "右栏 元信息 标题缺失:\n{all}");
        assert!(compact.contains("欢迎"), "中栏对话内容缺失:\n{all}");
        assert!(all.contains("dev"), "门客 role 应出现在左栏:\n{all}");
    }

    /// roster 上下移 + Enter 切 active。
    #[test]
    fn roster_up_down_then_enter_switches_active() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.upsert_roster(a, "dev".into(), ShelfStatus::Idle);
        // 进 roster focus
        app.focus = Focus::Roster;
        // 初始选中玄女（index 0）
        app.roster_state.select(Some(0));

        app.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(app.roster_state.selected(), Some(1));
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a));
        assert_eq!(app.focus, Focus::Input);
    }

    /// F2 toggle 事件流显示。
    #[test]
    fn f2_toggles_events_visible() {
        let mut app = ReplApp::stub();
        assert!(!app.events_visible);
        app.handle_key(KeyCode::F(2), KeyModifiers::empty());
        assert!(app.events_visible);
        app.handle_key(KeyCode::F(2), KeyModifiers::empty());
        assert!(!app.events_visible);
    }

    /// dialogue 桶容量上限——老的被挤出去。
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

    /// ThinkingStarted/Finished 切 task_meta.thinking。
    #[test]
    fn thinking_events_toggle_per_agent_flag() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.ingest(&mk_ev(Some(xid), EventKind::ThinkingStarted));
        assert!(app.task_meta.get(&xid).map(|m| m.thinking).unwrap_or(false));
        app.ingest(&mk_ev(Some(xid), EventKind::ThinkingFinished));
        assert!(!app.task_meta.get(&xid).map(|m| m.thinking).unwrap_or(true));
    }

    // ───────── PATH 探测：保证 `fuxi` binary 在玄女能调到的位置 ─────────

    #[test]
    fn require_fuxi_in_path_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path_env = dir.path().as_os_str();
        let res = require_fuxi_in_path("fuxi", Some(path_env));
        assert!(res.is_err(), "空 PATH 应当失败");
        let msg = res.unwrap_err().to_string();
        // error message 必须指向 install.sh，不让用户自己猜
        assert!(
            msg.contains("scripts/install.sh"),
            "error 必须指向 scripts/install.sh；实际：{msg}"
        );
    }

    #[test]
    fn require_fuxi_in_path_errors_when_path_env_unset() {
        let res = require_fuxi_in_path("fuxi", None);
        assert!(res.is_err(), "无 PATH env 也应当失败");
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
        // 写入但不给可执行位——chmod 默认 644
        std::fs::write(&bin_path, b"not exe").unwrap();
        let res = require_fuxi_in_path("fuxi", Some(dir.path().as_os_str()));
        assert!(res.is_err(), "无 +x 的文件不应被接受");
    }
}
