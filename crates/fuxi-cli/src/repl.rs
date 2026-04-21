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
use chrono::{DateTime, Local};
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

/// 当前主题——代理到 `theme::current()`，返 **值** 不是引用。
///
/// R10-integ 起 theme 可运行时切（`/theme <name>`），底层是 `RwLock<Theme>`；
/// 由于 `Theme: Copy` 返值比借 `RwLockReadGuard` 更不易死锁。
pub(crate) fn theme() -> Theme {
    theme::current()
}
use fuxi_orchestrator::{
    DEFAULT_TICK_INTERVAL_SECS, Fuxi, FuxiConfig, IdleGcTask, IdleShutdowner, ShelfStatus,
    SystemEventBridge, WorkerKind, ttl_from_env,
};
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
    /// 玄女的 role（roles/<role>/ROLE.md，兼容旧 skills/.../SKILL.md）。默认 `xuannv`。
    #[arg(long, default_value = "xuannv")]
    pub xuannv_role: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            // SAFETY: 字面量常量，IPv4 socket addr 文法保证可解析；Default trait 无法返 Result
            bind: "127.0.0.1:4100"
                .parse()
                .expect("hardcoded 127.0.0.1:4100 必能解析"),
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

/// 启动时是否打印 banner：
/// - `FUXI_BANNER=off`（任意大小写）→ 跳过
/// - stdout 非 tty（被管道 / 重定向）→ 跳过（避免污染脚本输出）
fn should_show_banner() -> bool {
    use std::io::IsTerminal;
    if let Ok(v) = std::env::var("FUXI_BANNER")
        && (v.eq_ignore_ascii_case("off") || v == "0")
    {
        return false;
    }
    std::io::stdout().is_terminal()
}

pub async fn run(args: Args) -> Result<()> {
    require_fuxi_in_path("fuxi", std::env::var_os("PATH").as_deref())?;

    // D17 · 启动 banner：进 TUI 之前打一下，alt-screen 会覆盖，但留 scrollback。
    // FUXI_BANNER=off 可跳过；stdout 非 tty（pipe / script）也跳过。
    if should_show_banner() {
        crate::banner::print_to_stdout(&crate::theme::from_env());
    }

    // M3.2 · ~/.fuxi/skills → ~/.fuxi/roles 一次性迁移（幂等；失败只 warn）
    if let Err(e) = fuxi_skills::migrate_user_dir() {
        tracing::warn!(error = %e, "M3.2 用户目录迁移出错，忽略继续");
    }

    if skill_loader::skills_root().is_none() {
        return Err(anyhow!(
            "找不到 roles 目录：试过 $FUXI_ROLES_DIR / $FUXI_SKILLS_DIR / git-root/roles / ./roles / $HOME/.fuxi/roles（及 skills/ 旧名 fallback）都不在。\n\
             建议：export FUXI_ROLES_DIR=/Users/e0_7/fuxi/roles（或把 fuxi/roles 软链到 ~/.fuxi/roles）"
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

    // 门客 idle GC（M2.4）——每 30s 扫一次 shelf，超时 idle 门客回收。
    // Arc<Fuxi> 实现 IdleShutdowner；GC 拿 Arc 持 'static，不阻止 Fuxi drop。
    let gc_shutdowner: Arc<dyn IdleShutdowner> = fuxi.clone();
    let gc_task = IdleGcTask::new(
        fuxi.clone_shelf(),
        gc_shutdowner,
        bus.clone(),
        ttl_from_env(),
        Duration::from_secs(DEFAULT_TICK_INTERVAL_SECS),
    )
    .spawn();

    // 玄女 cc session 续写：策府存哪一个 session_id 就 `--resume` 它，没有则
    // 首次新生成并回写。这样关了 fuxi 再开，玄女上下文不丢。见 `session.rs`。
    //
    // 策府要在 Daemon::new 前打开，因为 daemon 也需要它来处理 P2 召回 spawn flag
    // (`--recall-task` / `--recall-role`)。oracle 是 Arc<SqlitePool> 语义 clone 便宜，
    // repl 主线和 daemon 共享同一个 pool 不冲突。
    let memory_db = crate::memory_cmd::resolve_db_path(None).context("解析策府 DB 路径")?;
    let oracle = OracleStore::connect_file(&memory_db)
        .await
        .with_context(|| format!("连接策府 DB {}", memory_db.display()))?;

    // P2 召回入库——dispatch pump Done 时落 task-<id> + role-<role> 两条 fact。
    // why 在 daemon::new 之前：sink 写入路径独立于 daemon 查询路径，先注入再起 daemon
    // 保证一接客就能落库，不丢 race 窗口里的早期 task。
    fuxi.set_recall_sink(Arc::new(crate::recall_sink::OracleRecallSink::new(
        oracle.clone(),
    )))
    .await;
    let daemon = Daemon::new(
        fuxi.clone(),
        bus.clone(),
        sched_store.clone(),
        keeper,
        oracle.clone(),
    );
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
        .with_context(|| format!("加载 roles/{}/ROLE.md", args.xuannv_role))?;
    let xuannv_profile = loaded.profile.clone();
    let (resume_session_id, session_id) = crate::session::resolve_xuannv_session(&oracle)
        .await
        .context("解析玄女 session_id")?;
    // 构造启动横幅——resume 时让用户看到"续上次对话"；首次则什么都不说
    let resume_banner: Option<String> = match (&resume_session_id, &session_id) {
        (Some(id), _) => {
            tracing::info!(session = %id, "玄女 cc 续写策府已有 session");
            let short = &id[..id.len().min(8)];
            Some(format!(
                "已续写玄女上次 session（{short}…）——上下文保留在 cc 端，TUI 对话历史不回放。直接开口接着聊。"
            ))
        }
        (_, Some(id)) => {
            tracing::info!(session = %id, "玄女 cc 首次启动，新 session_id 已落盘");
            None
        }
        // 上游契约：resolve_xuannv_session 总返回至少一个 Some。若两者皆 None，
        // 说明策府读写出了 invariant 异常——返 Err 而非 panic，让顶层错误边界打印。
        (None, None) => anyhow::bail!(
            "策府返回 (None, None)：玄女 session_id 既未命中也未生成，请检查 oracle_facts 表"
        ),
    };

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

    // M2.5 · 策府 Extractor 挂载：订阅 TaskStateChanged::Done，spawn extractor
    // 门客自动抽 S-P-O 入甲骨。skill 缺失时降级为跳过（warn 提示用户装）。
    // FUXI_EXTRACTOR_ENABLED=0 关。
    let oracle = Arc::new(oracle);
    let extractor_task = match crate::extractor_hook::load_extractor_launch() {
        Ok((ex_profile, ex_cfg)) => {
            let spawner = Arc::new(crate::extractor_hook::FuxiExtractorSpawner::new(
                fuxi.clone(),
                bus.clone(),
                ex_profile,
                ex_cfg,
            ));
            let extractor = fuxi_memory::Extractor::new(
                bus.clone(),
                oracle.clone(),
                spawner,
                crate::extractor_hook::extractor_cfg_from_env(),
            );
            tracing::info!("Extractor 已挂载——task Done 后自动抽 fact 入策府");
            Some(extractor.spawn())
        }
        Err(e) => {
            tracing::warn!(error = %e, "extractor skill 加载失败，长期记忆自动抽取禁用");
            None
        }
    };

    let greet = Task::new(
        "greet",
        "用户刚启动 fuxi REPL。请用一句话（十字以内）主动问好，邀请用户提需求。不要自我介绍。",
    );
    if let Err(e) = fuxi.dispatch(xuannv_id, greet).await {
        tracing::warn!(error = %e, "greet dispatch 失败，继续");
    }

    let outcome = drive_tui(bus, fuxi.clone(), xuannv_id, resume_banner).await;

    daemon_shutdown.notify_waiters();
    if let Err(e) = fuxi.shutdown().await {
        tracing::warn!(error = %e, "fuxi shutdown 部分失败");
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    hub_task.abort();
    daemon_task.abort();
    keeper_task.abort();
    gc_task.abort();
    bridge_task.abort();
    if let Some(t) = extractor_task {
        t.abort();
    }

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
    /// agent 自称——玄女 / 门客都用这种。`name` v1 渲染不再用（每个
    /// `ActiveTarget` 独立 bucket，身份靠输入框标题明示），但保留以备 v2
    /// 把事件流嵌入对话时做 per-msg 标签。
    Agent {
        #[allow(dead_code)]
        name: String,
        text: String,
    },
    System(String),
}

/// 对话区条目 = 时间戳 + 消息。
///
/// WHY 包一层：M4.1 方案 A 只给每条消息**首行**挂「`▍ HH:MM `」锚点，
/// 续行空白占位。时间戳不能从 `DialogueLine` 里算（纯数据），必须在
/// push 那一刻记下。
#[derive(Debug, Clone)]
pub(crate) struct DialogueEntry {
    pub at: DateTime<Local>,
    pub line: DialogueLine,
}

impl DialogueEntry {
    pub fn new(line: DialogueLine) -> Self {
        Self {
            at: Local::now(),
            line,
        }
    }

    #[cfg(test)]
    pub fn at_fixed(line: DialogueLine, hour: u32, minute: u32) -> Self {
        use chrono::TimeZone;
        let at = Local
            .with_ymd_and_hms(2026, 4, 21, hour, minute, 0)
            .single()
            .expect("test time");
        Self { at, line }
    }
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
    /// F3：toggle 鼠标捕获。关闭后终端回归 native select/copy；再按恢复。
    /// app 记状态，execute! 在 drive_tui 里执行（需要 terminal backend）。
    ToggleMouse,
}

/// REPL TUI 的核心状态。纯逻辑，不 own terminal——便于单测。
pub(crate) struct ReplApp {
    pub(crate) xuannv_id: AgentId,
    pub(crate) xuannv_status: ShelfStatus,
    pub(crate) xuannv_thinking: bool,

    pub(crate) focus: Focus,
    pub(crate) active: ActiveTarget,
    pub(crate) dialogues: HashMap<ActiveTarget, VecDeque<DialogueEntry>>,

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

    /// 鼠标捕获当前开关状态（默认 true）。F3 切；关闭后终端回 native select
    /// 复制——ratatui mouse capture 会吞 terminal 自带的选择行为，用户要
    /// 复制会话时按一下 F3 切 off，选中用 Cmd/Ctrl+C 复制后再按 F3 恢复。
    pub(crate) mouse_enabled: bool,

    /// 最近一次 Esc 按下的时刻。`None` = 无待确认的中断意图。
    /// WHY 双击 Esc 才中断：误触 Esc 太常见（tmux prefix / vim 习惯），
    /// 单击只回玄女或给提示——真中断要两次，在 `ESC_DOUBLE_WINDOW` 内。
    pub(crate) esc_last_at: Option<Instant>,
    /// 当前窗口内已按 Esc 的次数（0 / 1）。超窗或其他按键会重置回 0。
    pub(crate) esc_count: u8,

    /// 右上角通知栈——渲染浮在对话之上，TTL 到点自动 prune。
    /// WHY 放 ReplApp：toast 源是多路的（agent_dead / 粘贴确认 / 复制成功 / 未来
    /// /theme 切换反馈），在一个共享 stack 里统一生命周期最简单。
    pub(crate) toasts: crate::toast::ToastStack,

    /// 输入下沿"活状态行"专用的 spinner。每 draw 推进一帧。
    /// WHY 放 ReplApp：spinner 本身是 stateful（idx），需要跨帧持久。
    pub(crate) status_spinner: crate::spinner::Spinner,

    /// 玄女进入 busy/thinking 的起始时刻——用于活状态行显示 elapsed。
    /// None = idle（状态行显示静态 hint）。
    pub(crate) xuannv_busy_since: Option<Instant>,

    /// 已提交输入的环形历史（↑/↓ 翻看）。
    pub(crate) history: crate::prompt_history::PromptHistory,
    /// 切 active target 时保留未发送草稿；切回来时还原。
    pub(crate) stash: crate::draft_stash::DraftStash,

    /// 拖选起点 cell 坐标（Down(Left) 记下）；None = 未拖。
    pub(crate) selection_anchor: Option<(u16, u16)>,
    /// 拖选终点 cell 坐标（Drag 更新；Up 清）。
    pub(crate) selection_cursor: Option<(u16, u16)>,
    /// 最近一次 draw_dialogue 的区域——鼠标坐标 → 对话行索引需要它。
    pub(crate) last_dialogue_area: Option<Rect>,

    /// roster（任务 / 门客列表）overlay 开关。
    /// WHY overlay 而非常驻左栏：单栏主体让对话区拉满宽度（方案 R9），roster
    /// 平时收起来不干扰；用户 F4 临时看一眼就好。Esc 优先关 overlay。
    pub(crate) roster_overlay_open: bool,
    /// meta（active target 的元信息）overlay 开关。同上，F5 切。
    pub(crate) meta_overlay_open: bool,

    /// 斜杠命令浮层（#17 接入 #13 的 SlashPopup）。
    /// WHY 放 ReplApp：popup 有自己的状态（open/filter/selected），要跨多帧持久。
    pub(crate) popup: crate::autocomplete::SlashPopup,
    /// 命令注册表——popup 的候选源 + slash submit 的 action 源。
    /// 每次用完都调 `register_default()` 太浪费（R11 /help 测过也没事，但整合后
    /// popup 每次 filter 都要它，应存一份）。后续 /theme 插件想增删命令时改这个。
    pub(crate) cmd_registry: crate::command_registry::CommandRegistry,
}

/// 双击 Esc 的判定窗口。2s 太紧会让真想中断的用户按不上；太松会跟单击混。
pub(crate) const ESC_DOUBLE_WINDOW: Duration = Duration::from_secs(2);

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
            mouse_enabled: true,
            esc_last_at: None,
            esc_count: 0,
            toasts: crate::toast::ToastStack::new(),
            status_spinner: crate::spinner::Spinner::new(),
            xuannv_busy_since: None,
            history: crate::prompt_history::PromptHistory::default(),
            stash: crate::draft_stash::DraftStash::new(),
            selection_anchor: None,
            selection_cursor: None,
            last_dialogue_area: None,
            roster_overlay_open: false,
            meta_overlay_open: false,
            popup: crate::autocomplete::SlashPopup::new(),
            cmd_registry: crate::command_registry::register_default(),
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
        self.push_entry(target, DialogueEntry::new(line));
    }

    fn push_entry(&mut self, target: ActiveTarget, entry: DialogueEntry) {
        let bucket = self.dialogues.entry(target).or_default();
        if bucket.len() == DIALOGUE_CAP {
            bucket.pop_front();
        }
        bucket.push_back(entry);
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
                    // Toast 叠一份——对话里留审计轨迹，toast 抓即时注意。
                    // WHY 不只 toast：TTL 到期 toast 就没了，历史翻不回来。
                    let role = self.lookup_role(id);
                    self.toasts.push(
                        format!("{role} 下线：{cause}"),
                        crate::toast::ToastVariant::Error,
                        Duration::from_secs(6),
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
            // WHY 删除 TaskDelivered/TaskCancelled 分支（M3.6）：
            // 这俩孤儿变体已从 EventKind 移除——终态走上面 TaskStateChanged 分支
            // 中的 Done|Cancelled 已经处理了 prune_after。
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
            _ => {}
        }
    }

    /// 完成/取消后清理过期任务。由 drive_tui 每帧前调一次；测试里直接喂 Instant。
    pub(crate) fn tick(&mut self, now: Instant) {
        // Toast 到期 prune——draw 之前 prune 避免已死 toast 还闪一帧。
        self.toasts.prune(now);

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
                self.switch_active(ActiveTarget::Xuannv);
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
            self.refresh_xuannv_busy_anchor();
            return;
        }
        if let Some(r) = self.idle_workers.iter_mut().find(|r| r.id == id) {
            r.status = status;
        }
    }

    fn set_thinking(&mut self, id: AgentId, flag: bool) {
        if id == self.xuannv_id {
            self.xuannv_thinking = flag;
            self.refresh_xuannv_busy_anchor();
            return;
        }
        if let Some(t) = self.task_by_worker_mut(id) {
            t.thinking = flag;
        }
    }

    /// 根据当前 xuannv 是否 busy 更新 `xuannv_busy_since` 时间锚。
    /// 从 idle→busy：记下此刻；busy→idle：清空。busy 态内不重置（避免 elapsed 跳）。
    fn refresh_xuannv_busy_anchor(&mut self) {
        let busy = self.xuannv_thinking || matches!(self.xuannv_status, ShelfStatus::Busy);
        match (busy, self.xuannv_busy_since) {
            (true, None) => self.xuannv_busy_since = Some(Instant::now()),
            (false, Some(_)) => self.xuannv_busy_since = None,
            _ => {}
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
            let new_active = match row {
                PaneRow::Xuannv => ActiveTarget::Xuannv,
                PaneRow::Task(i) => ActiveTarget::Worker(self.tasks[*i].worker),
                PaneRow::Idle(i) => ActiveTarget::Worker(self.idle_workers[*i].id),
                PaneRow::IdleHeader => return,
            };
            self.switch_active(new_active);
            self.roster_state.select(Some(idx));
        }
    }

    /// Esc：速回玄女。
    pub(crate) fn reset_to_xuannv(&mut self) {
        self.switch_active(ActiveTarget::Xuannv);
        self.focus = Focus::Input;
        self.resync_roster_selection();
    }

    /// 切 active target 的统一入口：
    /// 1. 把当前 input 文本 stash 到旧 target；
    /// 2. `active = new`；
    /// 3. 用 `stash.pop(new)` 还原该 target 的草稿（没有则清空输入）。
    ///
    /// WHY 统一：多处触发（Tab/Esc/roster 点击/点栏切换），若各自 stash 会漏；
    /// `take_input` 会重建 textarea，所以这里得显式 pop+insert。
    pub(crate) fn switch_active(&mut self, new: ActiveTarget) {
        if self.active == new {
            return;
        }
        let cur_draft = self.input_text();
        self.stash.stash(self.active, cur_draft);
        self.active = new;
        // 清当前输入（重建新 textarea 清 cursor 状态）。
        self.input = new_textarea();
        if let Some(prev) = self.stash.pop(new) {
            self.input.insert_str(&prev);
        }
        // 切到新 target 时历史光标归零——避免拿上一段 target 的 history 指针。
        self.history.reset_cursor();
    }

    /// 当前 active 对象是否"在干活"——决定 Esc 的语义是中断还是回玄女。
    ///
    /// Xuannv：shelf Busy 或 thinking=true。
    /// Worker：对应 task 在跑（thinking 或 state 处于 Running/Delivered 前），
    /// 且未标 prune_after（被删队前的跑中 task 才算）。
    pub(crate) fn active_is_busy(&self) -> bool {
        match self.active {
            ActiveTarget::Xuannv => {
                self.xuannv_thinking || matches!(self.xuannv_status, ShelfStatus::Busy)
            }
            ActiveTarget::Worker(id) => self
                .tasks
                .iter()
                .filter(|t| t.worker == id && t.prune_after.is_none())
                .any(|t| {
                    // 终态 Done/Cancelled 走 prune_after 分支处理；此处 t.prune_after=None
                    // 基本等价于"活着的 task"。再显式挡一下 Done/Cancelled 作为保险。
                    t.thinking || !matches!(t.state, TaskState::Done | TaskState::Cancelled)
                }),
        }
    }

    /// 处理 Esc 键。
    ///
    /// 语义（按优先级）：
    /// 1. active 在忙 + 2s 内已按过 Esc → 视为"二按"，发中断请求（目前先 push_line）
    ///    并重置计数。
    /// 2. active 在忙 + 首次按 → 记时、计数 1，给出 hint 要求再按一次。
    /// 3. active idle → 维持旧行为回玄女。
    ///
    /// `now` 注入：测试用假时钟避免 sleep；生产传 `Instant::now()`。
    /// 如果 `text` 是本地 slash 命令（如 `/theme`）则处理掉并返 `true`；否则返 `false`
    /// 让调用方按普通消息往 agent 派。
    ///
    /// WHY 返 bool 而非 Option<Submit>：这些命令都是"本地副作用 + toast 反馈"类，
    /// 不生成 Xuannv / Worker 派发。调用方只需区分"吃了"/"没吃"。
    pub(crate) fn try_handle_slash_submit(&mut self, text: &str) -> bool {
        let Some(rest) = text.strip_prefix('/') else {
            return false;
        };
        // 命令名 + 可选一个 arg（简单 split——命令当前都只带 0-1 个参数）。
        let mut parts = rest.splitn(2, char::is_whitespace);
        let Some(cmd) = parts.next() else {
            return false;
        };
        let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());
        match cmd {
            "theme" => {
                self.execute_theme_command(arg);
                true
            }
            "help" => {
                self.execute_help_command();
                true
            }
            _ => false,
        }
    }

    /// `/help` handler：把 `CommandRegistry::render_help_markdown()` 结果按行
    /// 塞进当前 active 的对话 bucket（System 行），便于用户翻看同时不打扰 agent。
    pub(crate) fn execute_help_command(&mut self) {
        let text = self.cmd_registry.render_help_markdown();
        let target = self.active;
        for line in text.lines() {
            // 空行也推进去——markdown 的 blank line 是段落分隔，保留让观感清晰。
            self.push_line(target, DialogueLine::System(line.to_string()));
        }
    }

    /// 统一 action 路由——popup 吐 `Execute(action)` 时调这个。
    /// 没实装的命令推一条 System 行告知用户，避免"按 Enter 无反应"黑洞。
    pub(crate) fn run_command_action(&mut self, action: crate::command_registry::CommandAction) {
        use crate::command_registry::CommandAction;
        match action {
            CommandAction::Help => self.execute_help_command(),
            CommandAction::Theme(name) => self.execute_theme_command(name.as_deref()),
            CommandAction::Clear => {
                // /clear：清掉当前 active 的对话 bucket 而非清全部（主人通常只想清当前视图）。
                if let Some(bucket) = self.dialogues.get_mut(&self.active) {
                    bucket.clear();
                }
                self.dialogue_scroll = 0;
                self.dialogue_auto_scroll = true;
            }
            CommandAction::Quit => {
                self.should_quit = true;
            }
            CommandAction::Kill | CommandAction::Status => {
                // TODO(#后续)：/kill 和 /status 需要接 orchestrator 的 shelf API。
                // 占位：给用户一条 System 行说明未实装，避免静默吞按键。
                self.push_line(
                    self.active,
                    DialogueLine::System("（此命令尚未实装，敬请期待）".into()),
                );
            }
        }
    }

    /// `/theme` handler：
    /// - 无参 → toast 列出可选主题名（从 `list_themes()` + 内置兜底）
    /// - 带名 → `set_theme(name)` + toast 成功/失败
    pub(crate) fn execute_theme_command(&mut self, name: Option<&str>) {
        const TTL: Duration = Duration::from_secs(4);
        match name {
            None => {
                let mut names = crate::theme::list_themes();
                // 兜底：若 theme 目录没 `mocha`/`latte` 条目（release 分发场景），
                // 仍把内置两款补进去——`set_theme` 对这两个名字不经文件也能命中。
                for builtin in ["mocha", "latte"] {
                    if !names.iter().any(|n| n == builtin) {
                        names.push(builtin.to_string());
                    }
                }
                names.sort();
                names.dedup();
                let msg = if names.is_empty() {
                    "没有可用主题".to_string()
                } else {
                    format!("可选主题：{}", names.join("、"))
                };
                self.toasts.push(msg, crate::toast::ToastVariant::Info, TTL);
            }
            Some(n) => match crate::theme::set_theme(n) {
                Ok(()) => {
                    self.toasts.push(
                        format!("主题已切到 {n}"),
                        crate::toast::ToastVariant::Success,
                        TTL,
                    );
                }
                Err(e) => {
                    self.toasts.push(
                        format!("切主题失败：{e}"),
                        crate::toast::ToastVariant::Error,
                        TTL,
                    );
                }
            },
        }
    }

    fn handle_esc_at(&mut self, now: Instant) {
        if !self.active_is_busy() {
            self.esc_last_at = None;
            self.esc_count = 0;
            self.reset_to_xuannv();
            return;
        }

        let in_window = self
            .esc_last_at
            .map(|t| now.saturating_duration_since(t) <= ESC_DOUBLE_WINDOW)
            .unwrap_or(false);

        if in_window && self.esc_count >= 1 {
            // 二按确认——发中断请求（现阶段只通知用户，真中断 API 待 R1 真入装）。
            self.push_line(self.active, DialogueLine::System("⏹ 中断请求已发".into()));
            self.esc_last_at = None;
            self.esc_count = 0;
        } else {
            self.esc_last_at = Some(now);
            self.esc_count = 1;
            self.push_line(
                self.active,
                DialogueLine::System("再按一次 Esc 确认中断".into()),
            );
        }
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
        self.handle_key_at(code, mods, Instant::now())
    }

    /// handle_key 可测版本——把 `now` 外注入以便在 Esc 双击窗口逻辑下跑确定性测试。
    pub(crate) fn handle_key_at(
        &mut self,
        code: KeyCode,
        mods: KeyModifiers,
        now: Instant,
    ) -> Option<Submit> {
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

        // Esc 计数在非 Esc 按键来时归零——"再按一次 Esc"约束只在连续按 Esc 时生效。
        if !matches!(code, KeyCode::Esc) {
            self.esc_last_at = None;
            self.esc_count = 0;
        }

        // slash popup 优先——开着时吞所有键给 popup 的状态机，按 PopupEvent 路由。
        // WHY 优先于 overlay / 全局键：popup 是最 ephemeral 的输入通道，用户期待
        // 它"拦住所有输入"直到关闭；Esc 关 popup 也要最先响应。
        if self.popup.is_open() {
            let ev = self.popup.handle_key(code, mods, &self.cmd_registry);
            match ev {
                crate::autocomplete::PopupEvent::None => {}
                crate::autocomplete::PopupEvent::Close => {}
                crate::autocomplete::PopupEvent::Execute(action) => {
                    self.run_command_action(action);
                }
            }
            return None;
        }

        // 空输入 + 按 `/` → 开 popup，**不**把这个 `/` 塞到 textarea。
        // 非空输入（句中 `/`）不触发 popup——保持 textarea 正常行为。
        if matches!(code, KeyCode::Char('/'))
            && !mods.contains(KeyModifiers::CONTROL)
            && !mods.contains(KeyModifiers::ALT)
            && self.input_text().is_empty()
        {
            self.popup.open(&self.cmd_registry);
            return None;
        }

        // 全局键
        match code {
            KeyCode::Tab => {
                self.cycle_active_to_next();
                self.focus = Focus::Input;
                return None;
            }
            KeyCode::Esc => {
                // 优先顺序：popup > overlay > interrupt。
                // popup 已经在上面分支吃掉，这里到不了；overlay 其次，最后才是双击 Esc。
                if self.roster_overlay_open || self.meta_overlay_open {
                    self.roster_overlay_open = false;
                    self.meta_overlay_open = false;
                    return None;
                }
                self.handle_esc_at(now);
                return None;
            }
            KeyCode::F(2) => {
                self.events_visible = !self.events_visible;
                return None;
            }
            KeyCode::F(3) => {
                self.mouse_enabled = !self.mouse_enabled;
                return Some(Submit::ToggleMouse);
            }
            KeyCode::F(4) => {
                // roster overlay 切换——开时顺便把焦点切到 roster 以便 ↑↓ Enter 导航。
                self.roster_overlay_open = !self.roster_overlay_open;
                if self.roster_overlay_open {
                    self.meta_overlay_open = false;
                    self.focus = Focus::Roster;
                } else {
                    self.focus = Focus::Input;
                }
                return None;
            }
            KeyCode::F(5) => {
                // meta overlay 是只读展示——焦点不跟着转，保持在 input 方便继续打字。
                self.meta_overlay_open = !self.meta_overlay_open;
                if self.meta_overlay_open {
                    self.roster_overlay_open = false;
                }
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
            // End / Ctrl+End：跳到底 + 恢复 auto-follow。
            // WHY Ctrl+End 兜底：输入框非空时 End 走 textarea（行尾），
            // Ctrl+End 保底让用户始终能跳到底。
            KeyCode::End
                if self.input_text().is_empty() || mods.contains(KeyModifiers::CONTROL) =>
            {
                self.jump_to_bottom();
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
                // slash 命令拦截——/theme 等不走 Xuannv/Worker，直接在本地做。
                // WHY 在 Enter 路径里拦：popup（#17）还没接 repl；先走最小接线，
                // popup 接入后再把 `CommandAction::Theme(name)` 也路由到同一个 handler。
                if self.try_handle_slash_submit(trimmed) {
                    self.history.push(trimmed);
                    return None;
                }
                // 记一条历史——提交后 ↑ 能回翻本句。push 有连续去重。
                self.history.push(trimmed);
                match self.active {
                    ActiveTarget::Xuannv => Some(Submit::Xuannv(trimmed.to_string())),
                    ActiveTarget::Worker(id) => Some(Submit::Worker(id, trimmed.to_string())),
                }
            }
            // ↑/↓ · history 导航条件：输入框空 **或** 当前内容就是 history
            // 填上来的（cursor.is_some() 说明在历史链上）。
            // WHY 不只看空：翻到上一条填上后内容非空，继续 ↑ 还得生效；只有
            // 用户真在编辑（离开历史链）时才把 ↑/↓ 让给 textarea。
            KeyCode::Up if self.input_text().is_empty() || self.history.cursor().is_some() => {
                if let Some(prev) = self.history.up() {
                    let prev = prev.to_string();
                    self.input = new_textarea();
                    self.input.insert_str(&prev);
                }
                None
            }
            KeyCode::Down if self.history.cursor().is_some() => {
                let next = self.history.down().map(|s| s.to_string());
                self.input = new_textarea();
                if let Some(s) = next {
                    self.input.insert_str(&s);
                }
                None
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
        // Sticky：手动滚到底 → 自动恢复 auto-follow（下一条新消息贴底）。
        if new_scroll >= max {
            self.dialogue_auto_scroll = true;
        }
    }

    /// 跳到对话底部——手动或 End/Ctrl+End 触发。同时恢复 auto-follow。
    pub(crate) fn jump_to_bottom(&mut self) {
        self.dialogue_auto_scroll = true;
        self.dialogue_scroll = self
            .last_dialogue_total
            .saturating_sub(self.last_dialogue_view);
    }

    pub(crate) fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        // 新一帧：清空上帧的 click regions（hit-test 后者胜，所以最后 register
        // 的 pane 会命中——pane 区域互不重叠，谁先注册其实无关）。
        self.click.clear();

        // 活状态行的 spinner 每帧推进一次——draw 频率≈20Hz，合 braille 观感。
        if self.active_is_busy() {
            self.status_spinner.tick();
        }

        // R9 单栏主体：对话 + 输入 + 状态 垂直堆叠，撑满宽度。
        // roster / meta 挪到 F4/F5 overlay（中央浮层），events_visible 仍可 F2
        // 打开一条底部 events 横带（窄高）方便偶尔观察事件流。
        //
        // 为什么不保留左侧常驻 roster：对话是主操作区，28 列常驻太占画面；
        // overlay 让用户"查一眼"而非"一直看"，更符合实际使用分布。
        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(if self.events_visible { 10 } else { 0 }),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(f.area());
        self.draw_dialogue(f, root[0]);
        self.click.register(root[0], ClickAction::FocusDialogue);
        if self.events_visible {
            self.draw_events(f, root[1]);
        }
        self.draw_input(f, root[2]);
        self.click.register(root[2], ClickAction::FocusInput);
        self.draw_status(f, root[3]);

        // overlay 浮层——优先级：roster > meta（互斥，同时只一个开着）。
        // 渲染顺序：overlay 在 toast 之前——toast 始终最顶。
        if self.roster_overlay_open {
            self.draw_roster_overlay(f, f.area());
        } else if self.meta_overlay_open {
            self.draw_meta_overlay(f, f.area());
        }

        // slash popup——在 input 正上方贴条，40%-60% 屏宽居中。
        // 位置在 overlay 之后、toast 之前：overlay 盖底层，toast 最顶，popup 居中。
        if self.popup.is_open() {
            self.draw_popup(f, root[2]);
        }

        // Toast 最后画——要浮在所有 pane 和 overlay 之上。用 Clear 擦一小块底色
        // 再盖 Paragraph，避免下层文字透出来。
        self.draw_toasts(f);
    }

    /// 在 input_area 正上方渲染 SlashPopup。
    ///
    /// 宽度取屏幕 40%~60%，位置贴 input 顶边向上浮（popup 高度 = 候选行数 + 2 边框
    /// + 1 filter 展示行，最多占 15 行避免盖满对话）。
    fn draw_popup(&self, f: &mut ratatui::Frame<'_>, input_area: Rect) {
        let t = theme();
        let lines = self.popup.render_lines(&t);
        // 顶上加一行展示 filter，底下用候选——高度 = filter(1) + 边框(2) + 候选行数。
        let desired_rows = (lines.len() as u16).min(12).saturating_add(3);

        let screen = f.area();
        let width = (screen.width.saturating_mul(55) / 100).clamp(30, screen.width);
        let height = desired_rows.min(screen.height);
        let x = screen.x + (screen.width.saturating_sub(width)) / 2;
        // 贴 input 顶边向上——`input_area.y` 之上 `height` 行。
        let y = input_area.y.saturating_sub(height);
        let rect = Rect {
            x,
            y,
            width,
            height,
        };

        use ratatui::text::Line;
        use ratatui::widgets::{Block, Borders, Paragraph};
        f.render_widget(ratatui::widgets::Clear, rect);

        let title = format!(" {} ", self.popup.display_input());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(t.focus_border()))
            .title(title);

        // 第一行留给引导提示（空 Line 占位让外观更稳——如无候选时不至于坍缩）。
        let mut body: Vec<Line<'_>> = Vec::with_capacity(lines.len() + 1);
        if lines.is_empty() {
            body.push(Line::from("（无匹配命令）"));
        } else {
            body.extend(lines);
        }
        let para = Paragraph::new(body).block(block);
        f.render_widget(para, rect);
    }

    /// 中央浮层通用布局：取 area 的 `width_pct%` × `height_pct%`，居中对齐。
    /// WHY 分离函数：roster / meta overlay 共用，将来 /help overlay 也会用。
    fn overlay_rect(area: Rect, width_pct: u16, height_pct: u16) -> Rect {
        let w = (area.width as u32 * width_pct as u32 / 100).max(20) as u16;
        let h = (area.height as u32 * height_pct as u32 / 100).max(6) as u16;
        let w = w.min(area.width);
        let h = h.min(area.height);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn draw_roster_overlay(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rect = Self::overlay_rect(area, 40, 60);
        // Clear 擦掉底层像素——不然 roster 内容会和对话区叠加。
        f.render_widget(ratatui::widgets::Clear, rect);
        self.draw_roster(f, rect);
        // 点击 overlay 区等同"确认要对 roster 操作"——维持 FocusRoster 语义。
        // 注册顺序在 dialogue 之后 → hit_test 逆序扫，overlay 优先命中。
        self.click.register(rect, ClickAction::FocusRoster);
    }

    fn draw_meta_overlay(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rect = Self::overlay_rect(area, 40, 60);
        f.render_widget(ratatui::widgets::Clear, rect);
        self.draw_meta(f, rect);
    }

    /// 右上角 toast 层渲染。空 stack 是 no-op，不占用任何像素。
    /// WHY 右上角而非中心：右上不挡主操作区（对话 + 输入），也不盖左栏 task 树。
    fn draw_toasts(&self, f: &mut ratatui::Frame<'_>) {
        // 顶 2 行留给 block 边框；右边留到屏最右——toast 自己 render 会靠右贴边。
        let area = f.area();
        if area.width < 12 || area.height < 3 {
            return;
        }
        let toast_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        };
        for (rect, para) in self.toasts.render(&theme(), toast_area) {
            f.render_widget(ratatui::widgets::Clear, rect);
            f.render_widget(para, rect);
        }
    }

    /// 处理鼠标事件。v1 支持：
    ///
    /// - 左键按下 → pane focus 切换（`click.hit_test`）+ 若在对话区，记选择锚点
    /// - 左键拖拽 → 更新选择终点（仅对话区内生效）
    /// - 左键释放 → 若拖拽过且锚/终点都在对话区，抓文本入剪贴板 + toast
    /// - 滚轮 → 对话区滚动
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
                // 对话区内 Down：起一个拖选。
                if self.in_dialogue_area(ev.column, ev.row) {
                    self.selection_anchor = Some((ev.column, ev.row));
                    self.selection_cursor = Some((ev.column, ev.row));
                } else {
                    self.selection_anchor = None;
                    self.selection_cursor = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selection_anchor.is_some() {
                    // Drag 可能越出 dialogue area；终点位置自然 clamp 在 render 时做。
                    self.selection_cursor = Some((ev.column, ev.row));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let (Some(a), Some(b)) =
                    (self.selection_anchor.take(), self.selection_cursor.take())
                    && a != b
                {
                    self.finish_selection_copy(a, b);
                }
            }
            _ => {}
        }
    }

    fn in_dialogue_area(&self, col: u16, row: u16) -> bool {
        self.last_dialogue_area
            .map(|a| col >= a.x && col < a.x + a.width && row >= a.y && row < a.y + a.height)
            .unwrap_or(false)
    }

    /// 拖选释放后的收尾：抓选中行文本 → 剪贴板 → toast。
    fn finish_selection_copy(&mut self, anchor: (u16, u16), cursor: (u16, u16)) {
        let Some(area) = self.last_dialogue_area else {
            return;
        };
        let text = self.extract_selected_text(area, anchor, cursor);
        if text.is_empty() {
            return;
        }
        let char_count = text.chars().count();
        match crate::clipboard::copy_to_clipboard(&text) {
            Ok(()) => {
                self.toasts.push(
                    format!("已复制 {char_count} 字"),
                    crate::toast::ToastVariant::Success,
                    Duration::from_secs(3),
                );
            }
            Err(e) => {
                self.toasts.push(
                    format!("复制失败：{e}"),
                    crate::toast::ToastVariant::Error,
                    Duration::from_secs(5),
                );
            }
        }
    }

    /// 从当前对话 bucket 按"选中的 wrapped rows"抽出 plain text。
    ///
    /// 算法：
    /// 1. 重跑 `render_dialogue_collapsed` + 按 `count_wrapped_rows` 把每 Line
    ///    展开成"屏幕行"序列。
    /// 2. 计算锚点/终点 y 在 inner 区的**屏幕行**索引（含 scroll 偏移）。
    /// 3. 切片 [y_min..=y_max] 的 plain text join('\n')。
    ///
    /// 不精确到 char-level：v1 用户要精确选取可以关 F3 鼠标捕获走 native。
    pub(crate) fn extract_selected_text(
        &self,
        area: Rect,
        anchor: (u16, u16),
        cursor: (u16, u16),
    ) -> String {
        let inner_y = area.y + 1;
        let inner_h = area.height.saturating_sub(2);
        if inner_h == 0 {
            return String::new();
        }

        // Cell 坐标 → inner 屏幕行（clamp）。
        let to_row = |y: u16| -> u16 {
            if y < inner_y {
                0
            } else if y >= inner_y + inner_h {
                inner_h - 1
            } else {
                y - inner_y
            }
        };
        let row_a = to_row(anchor.1);
        let row_b = to_row(cursor.1);
        let (lo, hi) = if row_a <= row_b {
            (row_a, row_b)
        } else {
            (row_b, row_a)
        };

        // 对话 bucket 全量 Line 序列（与 draw 同路径）。
        let empty = VecDeque::new();
        let bucket = self.dialogues.get(&self.active).unwrap_or(&empty);
        let lines: Vec<Line<'_>> = render_dialogue_collapsed(bucket.iter());
        // 屏幕行展开：每 Line 重复 `count_wrapped_rows` 次（简化——把同 Line
        // 的续行内容一律归到首 Line 的文本；用户选到的 row 1 和 row 0 拿同一
        // plain text 没大问题，去重后即可）。
        let inner_w = area.width.saturating_sub(2);
        let mut rows_plain: Vec<String> = Vec::with_capacity(lines.len() * 2);
        for l in &lines {
            let plain: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            let wraps = count_wrapped_rows(l, inner_w).max(1);
            for i in 0..wraps {
                // 首屏幕行给完整 plain，后续续行给空串避免重复。
                rows_plain.push(if i == 0 { plain.clone() } else { String::new() });
            }
        }

        let scroll = self.dialogue_scroll as usize;
        let lo_idx = scroll + lo as usize;
        let hi_idx = scroll + hi as usize;
        let mut out: Vec<String> = Vec::new();
        for idx in lo_idx..=hi_idx {
            if let Some(s) = rows_plain.get(idx)
                && !s.is_empty()
            {
                out.push(s.clone());
            }
        }
        out.join("\n")
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
                    let marker = status_marker_span(self.xuannv_status);
                    let active_mark =
                        if active_row_idx == i && matches!(self.active, ActiveTarget::Xuannv) {
                            "▶ "
                        } else {
                            "  "
                        };
                    ListItem::new(Line::from(vec![
                        Span::raw(active_mark),
                        marker,
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
                        Span::styled("✓", Style::default().fg(theme().success()))
                    } else {
                        status_marker_span(task_state_to_shelf(t.state, t.thinking))
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
                    let (icon, title_color) =
                        task_icon_and_color(&t.title, t.prune_after.is_some());
                    ListItem::new(Line::from(vec![
                        Span::raw(active_mark),
                        Span::raw(icon),
                        Span::styled(
                            truncate_by_width(&title, 16),
                            Style::default().fg(title_color),
                        ),
                        Span::raw("  "),
                        marker,
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
                    let marker = status_marker_span(r.status);
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
                        marker,
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

        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        // 屏幕行（wrap 后）总数——用于 scroll 算底部对齐。
        let total: u16 = lines
            .iter()
            .map(|l| count_wrapped_rows(l, inner_w))
            .sum::<u16>()
            .max(1);
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

        // 记 area 以便鼠标 drag 把 cell 坐标 → 对话行映射。
        self.last_dialogue_area = Some(area);

        // 选中范围 overlay：对选中 cells 加 REVERSED modifier，用户视觉上看到
        // 被"反色"的选区。不精确（整行 cell）但对 v1 剪贴板够用。
        if let (Some(a), Some(b)) = (self.selection_anchor, self.selection_cursor) {
            apply_selection_reverse(f.buffer_mut(), area, a, b);
        }
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
        // Busy 态 → spinner + "调用中 · mm:ss"；Idle → 静态 hint。
        // WHY 两套：用户正在等回复时，状态行告诉他"活着、已等 X 秒"；
        // idle 时用不到进度，恢复 hint 教学按键。
        if self.active_is_busy() {
            let glyph = self.status_spinner.glyph();
            let elapsed = self.active_elapsed().map(humanize_elapsed);
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(glyph.to_string(), Style::default().fg(theme().info())),
                Span::raw(" 调用中"),
            ];
            if let Some(e) = elapsed {
                spans.push(Span::raw(" · "));
                spans.push(Span::styled(e, Style::default().fg(theme().dim_border())));
            }
            let para = Paragraph::new(Line::from(spans))
                .style(Style::default().fg(Color::Black).bg(Color::Gray));
            f.render_widget(para, area);
            return;
        }
        let hint = " Tab 循环 | Esc 回玄女 | F2 事件流 | F3 鼠标开关(复制用) | PgUp/PgDn 翻阅 | ⇧/⌥-Enter / ⌃-J 换行 | Enter 发送 | Ctrl-C 退出 ";
        let para = Paragraph::new(hint).style(Style::default().fg(Color::Black).bg(Color::Gray));
        f.render_widget(para, area);
    }

    /// 当前 active 对象忙了多久（Xuannv 看 busy_since；Worker 看 dispatched_at）。
    fn active_elapsed(&self) -> Option<Duration> {
        match self.active {
            ActiveTarget::Xuannv => self.xuannv_busy_since.map(|t| t.elapsed()),
            ActiveTarget::Worker(id) => self
                .tasks
                .iter()
                .filter(|t| t.worker == id && t.prune_after.is_none())
                .max_by_key(|t| t.dispatched_at)
                .map(|t| t.dispatched_at.elapsed()),
        }
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

/// 对话区首行锚点格式：「`▍ HH:MM `」= 竖条 1 + 空格 1 + 5 字 + 空格 1 = 8 宽。
/// 续行用 8 个空格对齐，视觉上续行"挂"在首行内容下方。
const ANCHOR_WIDTH: usize = 8;

/// 对话渲染（M4.1 方案 A · 2026-04-21）。
///
/// 每条 entry（不论多少内部换行）只给**首行**挂锚点：
/// ```
/// ▍ 14:32 你好，可以帮我...
///         继续这条消息的第二段   ← 续行空白占位对齐
///         第三段
///                                  ← entry 之间空行分隔
/// ▍ 14:32 玄女：好的，...
/// ```
///
/// WHY 首行锚点：老版本每行挂 `▍` 视觉噪音大 —— 用户长对话下"一片竖条"
/// 疲劳。方案 A 把锚点变稀，内容呼吸。时间戳帮助用户回溯。
///
/// WHY ANCHOR_WIDTH = 8：「▍」占 1 列 + 空格 + 「HH:MM」5 字 + 空格 = 8。
/// 续行缩进 8 空格 = 续行首字恰在首行首字下方。
///
/// WHY 不做 visual wrap 的缩进：ratatui `Paragraph::wrap` 自动把超长行折回
/// 最左列，伏羲不重写 wrap 层。hard newline 续行能对齐就够用，visual wrap
/// 边缘情形接受 flush-left。
fn render_dialogue_collapsed<'a, I>(iter: I) -> Vec<Line<'a>>
where
    I: IntoIterator<Item = &'a DialogueEntry>,
{
    let th = theme();
    let mut out = Vec::new();
    let mut first_entry = true;
    for entry in iter {
        if !first_entry {
            out.push(Line::from(""));
        }
        first_entry = false;

        match &entry.line {
            DialogueLine::User(t) => {
                push_anchored(
                    &mut out,
                    t,
                    th.user_first_line(),
                    false,
                    entry.at,
                    Style::default(),
                );
            }
            DialogueLine::Agent { name: _, text } => {
                push_anchored(
                    &mut out,
                    text,
                    th.agent_first_line(),
                    true,
                    entry.at,
                    Style::default(),
                );
            }
            DialogueLine::System(t) => {
                // System 消息弱存在感：锚点换成 `· ` muted，body italic warn。
                // 不挂时间戳（系统事件用户不关心发生在几点几分）。
                let mut first_line = true;
                for ln in t.lines() {
                    let prefix = if first_line { "· " } else { "  " };
                    first_line = false;
                    out.push(Line::from(vec![
                        Span::styled(prefix.to_string(), Style::default().fg(th.muted())),
                        Span::styled(
                            ln.to_string(),
                            Style::default()
                                .fg(th.warn())
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
            }
        }
    }
    out
}

/// 把多行 `text` 渲染为「首行锚点 + 续行缩进」的若干 `Line`。
///
/// - `anchor_color`: `▍` + 时间戳色
/// - `bold`: agent 首行加粗（品牌色更醒目）；user 不加粗
/// - `at`: 捕获 entry 的本地时间，打印 `HH:MM`
/// - `body_style`: 正文 Span 的基础 Style（目前默认；留给将来染色接口）
fn push_anchored<'a>(
    out: &mut Vec<Line<'a>>,
    text: &str,
    anchor_color: Color,
    bold: bool,
    at: DateTime<Local>,
    body_style: Style,
) {
    let anchor_text = format!("▍ {} ", at.format("%H:%M"));
    let anchor_style = {
        let mut s = Style::default().fg(anchor_color);
        if bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        s
    };
    let mut first_line = true;
    for ln in text.lines() {
        if first_line {
            out.push(Line::from(vec![
                Span::styled(anchor_text.clone(), anchor_style),
                Span::styled(ln.to_string(), body_style),
            ]));
            first_line = false;
        } else {
            out.push(Line::from(vec![
                Span::raw(" ".repeat(ANCHOR_WIDTH)),
                Span::styled(ln.to_string(), body_style),
            ]));
        }
    }
    // 单行（text 不含 \n）也要保证首行产生——上面循环对空串会跳过。
    if first_line {
        out.push(Line::from(vec![
            Span::styled(anchor_text, anchor_style),
            Span::styled(String::new(), body_style),
        ]));
    }
}

/// 估算一条逻辑 `Line` 在 `width` 宽度下被 `Paragraph.wrap` 后占用的屏幕行数。
///
/// 修 Bug 8（2026-04-20 用户测）：`Paragraph::scroll((y, 0))` + `.wrap()`
/// 下 `y` 按**屏幕行**算，但我们之前直接 `dialogue_scroll = lines.len() - inner_h`
/// 用逻辑行算，CJK/长消息被 wrap 后屏幕行>逻辑行 → 底部被切，用户看不见最新。
/// 按 unicode-width 算总宽再 ceil-div 屏宽，空行算 1（ratatui 不吃 0 行）。
fn count_wrapped_rows(line: &Line, width: u16) -> u16 {
    let total_width: usize = line
        .spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total_width == 0 {
        return 1;
    }
    let w = width.max(1) as usize;
    total_width.div_ceil(w) as u16
}

/// 把选中 cells 反色——给 draw 完的对话 buffer 叠 REVERSED modifier。
///
/// 选区语义：按 row 整行反色（从 min_y 到 max_y，整行宽）——终端文本
/// 选择的常见简化，用户实际要复制的也是"这几行"而非精确 char range。
/// `anchor`/`cursor` 是 Down/Drag 的 cell 坐标；被 clamp 到 `area`（借此
/// 避免选到边框之外）。
fn apply_selection_reverse(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    anchor: (u16, u16),
    cursor: (u16, u16),
) {
    if area.width < 3 || area.height < 3 {
        return;
    }
    // 避边框：inner = area 缩 1 圈。
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);
    if inner_w == 0 || inner_h == 0 {
        return;
    }

    let (y0, y1) = (anchor.1.min(cursor.1), anchor.1.max(cursor.1));
    // 限制在 inner 纵向区。
    let ys = y0.max(inner_y);
    let ye = y1.min(inner_y + inner_h - 1);
    if ys > ye {
        return;
    }

    for y in ys..=ye {
        for x in inner_x..(inner_x + inner_w) {
            if x < buf.area.width && y < buf.area.height {
                let cell = &mut buf[(x, y)];
                cell.modifier.insert(Modifier::REVERSED);
            }
        }
    }
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
        ShelfStatus::Idle => "●", // 单宽黑圆
        ShelfStatus::Busy => "◉", // 靶心
        ShelfStatus::Dead => "✕", // 叉号
    }
}

/// 状态 marker + theme 语义色一次到位——调用方省去 match 二次上色。
fn status_marker_span(s: ShelfStatus) -> Span<'static> {
    let th = theme();
    let color = match s {
        ShelfStatus::Idle => th.success(),
        ShelfStatus::Busy => th.info(),
        ShelfStatus::Dead => th.muted(),
    };
    Span::styled(status_marker(s), Style::default().fg(color))
}

/// D13 · 任务树节点 icon + 标题色。
///
/// - **user-turn**（Decision 04 退化：用户↔agent 的即时对话轮）→ `·` + muted
/// - **正式 task**（玄女派的活）→ `◇` + White（已完成则 muted）
///
/// WHY 这组 icon：`◇` 已是事件流 `task_created` 的 narrate icon，任务树
/// 复用同字 = 视觉同一语言；`·` 极简 1 列宽，恰好表达"临时/轻量"。两者
/// 都是单宽 ASCII/Unicode 非私用区字符，任何终端都不会豆腐。
fn task_icon_and_color(title: &str, pruned: bool) -> (&'static str, Color) {
    let is_user_turn = title == "user-turn";
    let icon = if is_user_turn { "· " } else { "◇ " };
    let color = if pruned || is_user_turn {
        theme().muted()
    } else {
        Color::White
    };
    (icon, color)
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

async fn drive_tui(
    bus: EventBus,
    fuxi: Arc<Fuxi>,
    xuannv_id: AgentId,
    resume_banner: Option<String>,
) -> Result<()> {
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
    if let Some(banner) = resume_banner {
        // resume 时的横幅：push 一条 System 消息到玄女 bucket，让用户立刻看到
        // "已续上"，不用自己去翻策府文件确认
        app.push_line(ActiveTarget::Xuannv, DialogueLine::System(banner));
    }
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
                                    // 2026-04-20 改：走 intervene 不走 dispatch。
                                    // 原实装每条消息 dispatch 新 user-turn task → 用户连发
                                    // 5 条变 5 个僵尸 task 堆左栏。intervene 路径：
                                    //  - idle → Decision 04 degrade 为单次 dispatch（正常）
                                    //  - busy → send_message 走 M2.1 pending queue（不起新 task）
                                    let fuxi_cl = fuxi.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = fuxi_cl.intervene(xuannv_id, false, &text).await {
                                            tracing::warn!(error = %e, "xuannv intervene 失败");
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
                                Some(Submit::ToggleMouse) => {
                                    // app.mouse_enabled 已在 handle_key 里被 toggle
                                    if app.mouse_enabled {
                                        let _ = execute!(
                                            terminal.backend_mut(),
                                            EnableMouseCapture
                                        );
                                    } else {
                                        let _ = execute!(
                                            terminal.backend_mut(),
                                            DisableMouseCapture
                                        );
                                    }
                                    // push 一条 system 提示用户当前模式
                                    let msg = if app.mouse_enabled {
                                        "鼠标捕获：开（滚轮/点击可用）"
                                    } else {
                                        "鼠标捕获：关（终端原生选中复制可用，F3 再切回）"
                                    };
                                    app.push_line(
                                        app.active,
                                        DialogueLine::System(msg.to_string()),
                                    );
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
    fn count_wrapped_rows_empty_line_is_one() {
        let line = Line::from(Span::raw(""));
        assert_eq!(count_wrapped_rows(&line, 10), 1);
    }

    #[test]
    fn count_wrapped_rows_short_line_is_one() {
        let line = Line::from(Span::raw("hi"));
        assert_eq!(count_wrapped_rows(&line, 10), 1);
    }

    #[test]
    fn count_wrapped_rows_long_ascii_wraps_by_width() {
        let line = Line::from(Span::raw("a".repeat(25)));
        // 25 / 10 = 3（ceil）
        assert_eq!(count_wrapped_rows(&line, 10), 3);
    }

    /// 关键防线：CJK 字符是 2 宽的，算错会导致对话滚动吞最新。
    #[test]
    fn count_wrapped_rows_cjk_counts_double_width() {
        // 5 个中文 = 10 显示宽度；屏宽 10 刚好 1 行
        let line = Line::from(Span::raw("玄女派门客去"));
        // "玄女派门客去" = 6 字符 × 2 宽 = 12 宽
        assert_eq!(count_wrapped_rows(&line, 10), 2, "12 宽/10 屏宽 应 ceil=2");
    }

    #[test]
    fn count_wrapped_rows_multi_span_sums_widths() {
        let line = Line::from(vec![
            Span::raw("▍ "),          // 2 宽（▍ + 空格）
            Span::raw("hello world"), // 11 宽
        ]);
        // 总 13 宽 / 10 = 2 行
        assert_eq!(count_wrapped_rows(&line, 10), 2);
    }

    #[test]
    fn narrate_event_covers_cc() {
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
    fn f3_toggles_mouse_enabled_and_emits_submit() {
        let mut app = ReplApp::stub();
        assert!(app.mouse_enabled, "默认 true");
        let out = app.handle_key(KeyCode::F(3), KeyModifiers::empty());
        assert_eq!(out, Some(Submit::ToggleMouse));
        assert!(!app.mouse_enabled, "按一次 F3 应关闭");
        let out = app.handle_key(KeyCode::F(3), KeyModifiers::empty());
        assert_eq!(out, Some(Submit::ToggleMouse));
        assert!(app.mouse_enabled, "再按 F3 应恢复");
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

    // ───────── #6 R1 · 双击 Esc 中断 ─────────

    fn dialogue_has_text(app: &ReplApp, target: ActiveTarget, needle: &str) -> bool {
        app.dialogues
            .get(&target)
            .map(|bucket| {
                bucket.iter().any(|e| match &e.line {
                    DialogueLine::System(s) => s.contains(needle),
                    DialogueLine::User(s) => s.contains(needle),
                    DialogueLine::Agent { text, .. } => text.contains(needle),
                })
            })
            .unwrap_or(false)
    }

    #[test]
    fn single_esc_with_active_task_sets_count_and_shows_hint() {
        let mut app = ReplApp::stub();
        // 玄女进入 thinking（busy）状态——active 默认就是 Xuannv。
        app.xuannv_thinking = true;
        let now = Instant::now();

        app.handle_key_at(KeyCode::Esc, KeyModifiers::empty(), now);

        assert_eq!(app.esc_count, 1, "首按 Esc 应记 1");
        assert!(app.esc_last_at.is_some(), "首按 Esc 应记时间戳");
        assert_eq!(
            app.active,
            ActiveTarget::Xuannv,
            "active 忙时单 Esc 不应回玄女（本来就是玄女，这里验证不抹 active）"
        );
        assert!(
            dialogue_has_text(&app, ActiveTarget::Xuannv, "再按一次 Esc"),
            "首按 Esc 应 push hint"
        );
    }

    #[test]
    fn double_esc_within_2s_sends_interrupt() {
        let mut app = ReplApp::stub();
        app.xuannv_thinking = true;
        let t0 = Instant::now();

        app.handle_key_at(KeyCode::Esc, KeyModifiers::empty(), t0);
        assert_eq!(app.esc_count, 1);

        // 1.5s 后二按——在 2s 窗口内。
        let t1 = t0 + Duration::from_millis(1500);
        app.handle_key_at(KeyCode::Esc, KeyModifiers::empty(), t1);

        assert_eq!(app.esc_count, 0, "二按确认后应重置计数");
        assert!(app.esc_last_at.is_none(), "二按确认后应清时间戳");
        assert!(
            dialogue_has_text(&app, ActiveTarget::Xuannv, "中断请求已发"),
            "二按应 push 中断确认消息"
        );
    }

    #[test]
    fn esc_timeout_resets() {
        let mut app = ReplApp::stub();
        app.xuannv_thinking = true;
        let t0 = Instant::now();

        app.handle_key_at(KeyCode::Esc, KeyModifiers::empty(), t0);
        assert_eq!(app.esc_count, 1);

        // 2.5s 后——超 2s 窗口；视作新一轮"首按"。
        let t1 = t0 + Duration::from_millis(2500);
        app.handle_key_at(KeyCode::Esc, KeyModifiers::empty(), t1);

        // 仍是 1——窗口超时后的按键起了新轮，不是二按。
        assert_eq!(app.esc_count, 1, "超窗后再按应作为新轮首按");
        assert!(
            !dialogue_has_text(&app, ActiveTarget::Xuannv, "中断请求已发"),
            "超窗不应发中断"
        );
    }

    #[test]
    fn ctrl_c_exits() {
        let mut app = ReplApp::stub();
        // 首 Ctrl-C：打 confirm，不退。
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.should_quit, "单 Ctrl-C 不退");
        assert!(app.confirm_quit);

        // 紧接第二 Ctrl-C：退。
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit, "双 Ctrl-C 应退");
    }

    #[test]
    fn other_key_between_esc_resets_count() {
        let mut app = ReplApp::stub();
        app.xuannv_thinking = true;
        let t0 = Instant::now();

        app.handle_key_at(KeyCode::Esc, KeyModifiers::empty(), t0);
        assert_eq!(app.esc_count, 1);

        // 插一个普通按键——应清掉 esc 计数。
        app.handle_key_at(KeyCode::Char('x'), KeyModifiers::empty(), t0);
        assert_eq!(app.esc_count, 0, "Esc 后按普通键应重置计数");

        // 再 Esc 是新"首按"。
        app.handle_key_at(
            KeyCode::Esc,
            KeyModifiers::empty(),
            t0 + Duration::from_millis(100),
        );
        assert!(
            !dialogue_has_text(&app, ActiveTarget::Xuannv, "中断请求已发"),
            "被普通键打断后不应视作二按"
        );
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

    // ───────── #7 R4 · Sticky 底部滚动 ─────────

    #[test]
    fn new_msg_during_scroll_doesnt_force_bottom() {
        // 用户 PgUp 滚上去后，新消息推入不得把 scroll 强拉到底。
        // 关键机制：draw_dialogue 仅在 dialogue_auto_scroll=true 时才把
        // scroll 置到 total-view；auto=false 时保持 scroll 不动。
        let mut app = ReplApp::stub();
        app.last_dialogue_total = 100;
        app.last_dialogue_view = 10;
        app.dialogue_scroll = 90;

        app.handle_key(KeyCode::PageUp, KeyModifiers::empty());
        let frozen_scroll = app.dialogue_scroll;
        assert!(!app.dialogue_auto_scroll, "PgUp 应冻结 auto");

        // 喂 5 条新对话（模拟 agent 回复堆积）。
        for i in 0..5 {
            app.push_line(
                ActiveTarget::Xuannv,
                DialogueLine::System(format!("新消息 {i}")),
            );
        }
        // push_line 本身不碰 scroll——auto=false 时 draw_dialogue 也不会把它拉底。
        assert!(!app.dialogue_auto_scroll, "新消息不应把 auto 强开");
        assert_eq!(
            app.dialogue_scroll, frozen_scroll,
            "auto=false 时 push_line 不应动 scroll"
        );
    }

    #[test]
    fn end_resumes_auto_follow() {
        let mut app = ReplApp::stub();
        app.last_dialogue_total = 200;
        app.last_dialogue_view = 20;
        app.dialogue_scroll = 50;
        app.dialogue_auto_scroll = false;

        app.handle_key(KeyCode::End, KeyModifiers::empty());
        assert!(app.dialogue_auto_scroll, "End 必须恢复 auto-follow");
        assert_eq!(
            app.dialogue_scroll, 180,
            "End 应把 scroll 置到 total - view"
        );
    }

    #[test]
    fn ctrl_end_jumps_to_bottom_even_when_input_nonempty() {
        let mut app = ReplApp::stub();
        app.last_dialogue_total = 200;
        app.last_dialogue_view = 20;
        app.dialogue_scroll = 0;
        app.dialogue_auto_scroll = false;
        // 输入非空——裸 End 会走 textarea；这时 Ctrl+End 必须兜底。
        app.input.insert_str("half written draft");

        app.handle_key(KeyCode::End, KeyModifiers::CONTROL);
        assert!(app.dialogue_auto_scroll, "Ctrl+End 应恢复 auto");
        assert_eq!(app.dialogue_scroll, 180);
    }

    // ───────── #10 R12-integ · drag-release 复制 ─────────

    fn stub_with_dialogue_area(lines: &[&str]) -> ReplApp {
        let mut app = ReplApp::stub();
        for line in lines {
            app.push_line(
                ActiveTarget::Xuannv,
                DialogueLine::System((*line).to_string()),
            );
        }
        // 模拟一次 draw 后的 area 状态——方便 mouse hit test 测试。
        // dialogue area 常规 60x20，起点 (28, 0)。
        app.last_dialogue_area = Some(Rect::new(28, 0, 60, 20));
        app.last_dialogue_view = 18;
        app.last_dialogue_total = 5;
        app
    }

    #[test]
    fn drag_single_cell_records_anchor_and_cursor() {
        let mut app = stub_with_dialogue_area(&["hello", "world"]);
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        assert_eq!(app.selection_anchor, Some((30, 5)));
        assert_eq!(app.selection_cursor, Some((30, 5)));

        app.handle_mouse(mk_mouse(MouseEventKind::Drag(MouseButton::Left), 50, 6));
        assert_eq!(app.selection_anchor, Some((30, 5)));
        assert_eq!(app.selection_cursor, Some((50, 6)));
    }

    #[test]
    fn release_without_drag_skips_copy() {
        let mut app = stub_with_dialogue_area(&["abc"]);
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 30, 5));
        // 同点 Up——单击不是拖拽；不触发复制。
        app.handle_mouse(mk_mouse(MouseEventKind::Up(MouseButton::Left), 30, 5));
        assert_eq!(app.selection_anchor, None);
        assert_eq!(app.selection_cursor, None);
        assert_eq!(app.toasts.len(), 0, "单击不应产 toast");
    }

    #[test]
    fn drag_release_clears_selection_state() {
        // 真跑一次 Down→Drag→Up，验证 state 清理。copy 副作用会打 stdout，
        // 测试进程接受——macOS pbcopy 存在，OSC52 写 stdout 不 fail。
        let mut app = stub_with_dialogue_area(&["hello world", "from fuxi"]);
        // scroll=0，行 1 对应 lines[0]="hello world"。
        app.dialogue_scroll = 0;
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 29, 1));
        app.handle_mouse(mk_mouse(MouseEventKind::Drag(MouseButton::Left), 45, 2));
        app.handle_mouse(mk_mouse(MouseEventKind::Up(MouseButton::Left), 45, 2));

        assert_eq!(app.selection_anchor, None, "Up 后锚点应清");
        assert_eq!(app.selection_cursor, None, "Up 后终点应清");
        // 至少一条 toast——Success 或（若 pbcopy 不在）Error。
        assert!(!app.toasts.is_empty(), "Up 应 push toast");
    }

    #[test]
    fn extract_selected_text_joins_rows() {
        let mut app = stub_with_dialogue_area(&["alpha", "beta", "gamma"]);
        app.dialogue_scroll = 0;
        let area = app.last_dialogue_area.unwrap();
        // 选 row 1..row 3（inner y: 1,2,3 → inner rows 0,1,2）。
        // render_dialogue_collapsed 对 3 条 entry 产出 5 行（3 消息 + 2 分隔空行）。
        let text = app.extract_selected_text(area, (30, 1), (30, 3));
        assert!(text.contains("alpha"), "应含选中首行；实得: {text}");
    }

    #[test]
    fn drag_outside_dialogue_area_does_not_start_selection() {
        let mut app = stub_with_dialogue_area(&["x"]);
        // 在对话 area 外（y=25，area.height=20 从 y=0 起）点一下。
        app.handle_mouse(mk_mouse(MouseEventKind::Down(MouseButton::Left), 30, 25));
        assert_eq!(app.selection_anchor, None, "对话区外 Down 不应起选");
    }

    // ───────── #8 R5-integ · 历史 + stash 接入 ─────────

    #[test]
    fn sending_adds_to_history() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        let _ = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(app.history.len(), 1, "Enter 提交应入历史");
    }

    #[test]
    fn up_recalls_last_when_input_empty() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert!(app.input_text().is_empty(), "提交后输入应清空");

        app.handle_key(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(app.input_text(), "x", "输入空时 ↑ 应回填上条");
    }

    #[test]
    fn up_does_nothing_when_input_nonempty() {
        // 用户正在编辑时 ↑ 不应吞内容——交给 textarea。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        app.handle_key(KeyCode::Char('w'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('p'), KeyModifiers::empty());

        app.handle_key(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(app.input_text(), "wip", "非空时 ↑ 不应覆盖");
    }

    #[test]
    fn down_returns_to_live_draft() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());

        // Up 两次：b → a；再 Down 应回到 b；再 Down 回到实时（空）。
        app.handle_key(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(app.input_text(), "b");
        app.handle_key(KeyCode::Up, KeyModifiers::empty());
        assert_eq!(app.input_text(), "a");
        app.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(app.input_text(), "b");
        app.handle_key(KeyCode::Down, KeyModifiers::empty());
        assert_eq!(app.input_text(), "", "翻到实时应清空");
    }

    #[test]
    fn switch_target_preserves_draft() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "scout".into(),
                cli: "cc".into(),
            },
        ));

        // 在玄女面板打半句话——不提交。
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        assert_eq!(app.input_text(), "hi");

        // 切到 worker a（Tab）。
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a));
        assert_eq!(app.input_text(), "", "切到新 target 应清输入框");

        // 在 worker 面板打另半句。
        app.handle_key(KeyCode::Char('y'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('o'), KeyModifiers::empty());

        // 切回玄女——应还原 "hi"。
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Xuannv);
        assert_eq!(app.input_text(), "hi", "切回应还原原 target 草稿");

        // 再切回 worker——应还原 "yo"。
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a));
        assert_eq!(app.input_text(), "yo");
    }

    #[test]
    fn stash_empty_draft_clears() {
        // 用户故意删光再切走——不应留残留。
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "scout".into(),
                cli: "cc".into(),
            },
        ));
        app.handle_key(KeyCode::Char('z'), KeyModifiers::empty());
        app.handle_key(KeyCode::Backspace, KeyModifiers::empty());
        assert_eq!(app.input_text(), "");
        app.handle_key(KeyCode::Tab, KeyModifiers::empty()); // 切到 a
        app.handle_key(KeyCode::Tab, KeyModifiers::empty()); // 切回
        assert_eq!(app.input_text(), "", "空草稿切走再回来应仍空");
    }

    // ───────── #9 R6 · 输入下沿活状态行 ─────────

    #[test]
    fn status_idle_shows_static_hint() {
        let mut app = ReplApp::stub();
        // 默认 idle → active_is_busy = false
        assert!(!app.active_is_busy());
        let backend = TestBackend::new(120, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();

        // 底部 hint 行含 "Tab 循环" 关键字。
        let last = row_text(&buf, 23);
        let compact: String = last.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("Tab") && compact.contains("循环"),
            "idle hint 应含 'Tab 循环'；实得: {last:?}"
        );
    }

    #[test]
    fn status_busy_shows_spinner_and_elapsed() {
        let mut app = ReplApp::stub();
        app.xuannv_thinking = true;
        app.refresh_xuannv_busy_anchor();
        // 保证 active_elapsed 走得到——busy_since 设 5s 前。
        app.xuannv_busy_since = Some(Instant::now() - Duration::from_secs(5));

        let backend = TestBackend::new(120, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();

        let last = row_text(&buf, 23);
        let compact: String = last.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("调用中"),
            "busy 状态行应含 '调用中'；实得: {last:?}"
        );
        // elapsed 至少含"5" 或 "s"——humanize_elapsed 对 5 秒返 "5s"。
        assert!(
            compact.contains("5s") || compact.contains("5"),
            "busy 状态行应含 elapsed；实得: {last:?}"
        );
    }

    #[test]
    fn status_transitions_swap_correctly() {
        // busy → idle 切回 hint；再 busy 回来再切 spinner。
        let mut app = ReplApp::stub();
        let t0 = Instant::now();
        app.xuannv_thinking = true;
        app.xuannv_busy_since = Some(t0);
        assert!(app.active_is_busy());

        app.xuannv_thinking = false;
        app.refresh_xuannv_busy_anchor();
        assert!(!app.active_is_busy());
        assert!(app.xuannv_busy_since.is_none(), "idle 后应清 busy_since");

        // 再 busy：busy_since 要重记（不能继承上轮）。
        app.xuannv_thinking = true;
        app.refresh_xuannv_busy_anchor();
        assert!(app.xuannv_busy_since.is_some(), "再 busy 应重记 busy_since");
    }

    // ───────── #11 R3-integ · Toast 接入 repl ─────────

    #[test]
    fn agent_dead_event_creates_error_toast() {
        let mut app = ReplApp::stub();
        let a = AgentId::new();
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "scout".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentDead {
                cause: "WS EOF".into(),
            },
        ));
        assert_eq!(app.toasts.len(), 1, "AgentDead 应叠一条 toast 到 stack");
        let t = app.toasts.iter().next().expect("toast 存在");
        assert_eq!(t.variant, crate::toast::ToastVariant::Error);
        assert!(
            t.text.contains("scout") && t.text.contains("WS EOF"),
            "toast 文本应含 role + cause：{}",
            t.text
        );
    }

    #[test]
    fn toast_prunes_on_tick() {
        let mut app = ReplApp::stub();
        // 直接 push 一条已过期的 toast——ttl 1ms + created_at=very_old。
        let anchor = Instant::now() - Duration::from_secs(10);
        app.toasts.push(
            "stale",
            crate::toast::ToastVariant::Info,
            Duration::from_millis(1),
        );
        // 手动把 created_at 撤到很早 —— push 用 now()，我们换条 via 直接操作
        // 不便（ToastStack 只暴露 push）。改法：塞一条 ttl 极短的再 tick。
        app.tick(Instant::now() + Duration::from_secs(5));
        assert_eq!(app.toasts.len(), 0, "tick 应 prune 过期 toast");
        let _ = anchor; // 压警告
    }

    #[test]
    fn toast_renders_over_content() {
        // render 路径不 panic 且能返回对应 rect。这里走完整 draw 检查 buffer
        // 里右上区域含有 toast 文本。
        let mut app = ReplApp::stub();
        app.toasts.push(
            "测试提示",
            crate::toast::ToastVariant::Info,
            Duration::from_secs(30),
        );
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();

        // ratatui TestBackend 对 CJK wide char 占 2 cells：后半 cell 存空格。
        // row_text 直接 concat 会出现"测 试 提 示"的 gap——compact 后匹配。
        let compact = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
        let mut found = false;
        let mut dump = String::new();
        for y in 0..buf.area.height {
            let row = row_text(&buf, y);
            dump.push_str(&format!("{y:02}|{row}\n"));
            if compact(&row).contains("测试提示") {
                found = true;
            }
        }
        assert!(found, "toast 文本应出现在 buffer 中，dump:\n{dump}");
    }

    #[test]
    fn reaching_bottom_manually_resumes_auto() {
        // 用户 PgUp 后又 PgDn 到底 → 自动回 auto-follow。
        let mut app = ReplApp::stub();
        app.last_dialogue_total = 100;
        app.last_dialogue_view = 10;
        app.dialogue_scroll = 90; // 起点已在底
        app.handle_key(KeyCode::PageUp, KeyModifiers::empty());
        assert!(!app.dialogue_auto_scroll);
        // 反复 PgDn 把 scroll 拉回最大值（90）→ auto 自动 true。
        for _ in 0..20 {
            app.handle_key(KeyCode::PageDown, KeyModifiers::empty());
        }
        assert_eq!(app.dialogue_scroll, 90, "应到 total-view");
        assert!(app.dialogue_auto_scroll, "手动滚到底应恢复 auto");
    }

    // ───────── 对话渲染 · M4.1 方案 A ─────────

    /// 每 entry 只首行挂锚点，多 entry 之间插空 Line 分隔。
    #[test]
    fn render_dialogue_v2_each_entry_has_one_anchor() {
        let entries = [
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "玄女".into(),
                    text: "line1".into(),
                },
                14,
                32,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "玄女".into(),
                    text: "line2".into(),
                },
                14,
                33,
            ),
        ];
        let rendered = render_dialogue_collapsed(entries.iter());
        // 2 entry + 1 空行分隔 = 3 行
        assert_eq!(rendered.len(), 3, "应 = 2 entry + 1 分隔 = 3");
        assert!(line_to_plain(&rendered[0]).starts_with("▍ 14:32 "));
        assert_eq!(line_to_plain(&rendered[1]), "", "entry 之间应 blank 分隔");
        assert!(line_to_plain(&rendered[2]).starts_with("▍ 14:33 "));
    }

    /// hard newline 续行：首行挂锚点，后续行 8 空格对齐，不重复挂 `▍`。
    #[test]
    fn render_dialogue_v2_first_line_prefix_only() {
        let entries = [DialogueEntry::at_fixed(
            DialogueLine::User("第一段\n第二段\n第三段".into()),
            9,
            5,
        )];
        let rendered = render_dialogue_collapsed(entries.iter());
        assert_eq!(rendered.len(), 3);
        assert!(line_to_plain(&rendered[0]).starts_with("▍ 09:05 "));
        assert!(
            !line_to_plain(&rendered[1]).contains('▍'),
            "第 2 段不应重复竖条: {:?}",
            line_to_plain(&rendered[1])
        );
        assert!(
            line_to_plain(&rendered[1]).starts_with("        "),
            "8 空格对齐"
        );
        assert!(line_to_plain(&rendered[2]).starts_with("        "));
    }

    /// CJK 宽度：首行锚点固定 8 宽，不随内容 CJK 膨胀。
    #[test]
    fn render_dialogue_v2_cjk_width() {
        let entries = [DialogueEntry::at_fixed(
            DialogueLine::User("你好世界\n续行内容".into()),
            0,
            0,
        )];
        let rendered = render_dialogue_collapsed(entries.iter());
        // 首行前缀按 unicode-width 应为 `▍ 00:00 ` = 1 + 1 + 5 + 1 = 8 cells
        let first_prefix_width = UnicodeWidthStr::width("▍ 00:00 ");
        assert_eq!(first_prefix_width, 8, "锚点宽度 = 8");
        // 续行 8 空格
        let second = line_to_plain(&rendered[1]);
        let leading_spaces: usize = second.chars().take_while(|c| *c == ' ').count();
        assert_eq!(leading_spaces, 8, "续行缩进 8 = 和锚点同宽");
    }

    /// 续行首字与首行首字同列——视觉"挂"在首行内容下方。
    #[test]
    fn render_dialogue_v2_indent_alignment() {
        let entries = [DialogueEntry::at_fixed(
            DialogueLine::Agent {
                name: "".into(),
                text: "A\nB".into(),
            },
            12,
            0,
        )];
        let rendered = render_dialogue_collapsed(entries.iter());
        let first = line_to_plain(&rendered[0]);
        let second = line_to_plain(&rendered[1]);
        // 首行 A 的列 = 锚点宽度后第一字符位置
        let first_content_col = UnicodeWidthStr::width(&first[..first.find('A').unwrap()]);
        let second_content_col = UnicodeWidthStr::width(&second[..second.find('B').unwrap()]);
        assert_eq!(
            first_content_col, second_content_col,
            "续行首字应与首行首字同列"
        );
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
        assert!(matches!(w_bucket[0].line, DialogueLine::User(_)));
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

    // ───────── 单栏主体 + overlay snapshot ─────────

    /// R9 之后：默认 snapshot 只有对话 + 输入 + 状态，没有左右栏。
    #[test]
    fn single_column_snapshot_contains_dialogue_and_input() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
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
        let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();

        // 对话内容要在。
        assert!(compact.contains("玄女"), "玄女 字样缺失:\n{all}");
        assert!(compact.contains("欢迎"), "对话内容缺失:\n{all}");
        // overlay 关闭时 roster 的 "任务" / "空闲门客" 不该出现。
        assert!(
            !compact.contains("任务"),
            "overlay 关闭时 roster 字样不该出现:\n{all}"
        );
        assert!(
            !compact.contains("空闲门客"),
            "overlay 关闭时 idle header 不该出现:\n{all}"
        );
    }

    /// F4 打开 roster overlay 后，roster 栏字样应回到画面上。
    #[test]
    fn roster_overlay_snapshot_shows_roster_when_open() {
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
        app.roster_overlay_open = true;

        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let all: String = (0..buf.area.height)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("任务"),
            "overlay 开时应看到 任务 标题:\n{all}"
        );
        assert!(
            compact.contains("空闲门客"),
            "overlay 开时应看到 idle header:\n{all}"
        );
        assert!(all.contains("dev"), "overlay 开时应看到 role:\n{all}");
    }

    // ───────── R9 契约：单栏布局 + F4/F5 overlay + Esc 优先关 ─────────

    #[test]
    fn default_layout_is_single_column_plus_input() {
        // 默认没打 overlay 时：画面上没有 "任务"（roster 标题）/「关于」类 meta 标题。
        // 只应看到对话 + 输入 + 状态底条。
        let mut app = ReplApp::stub();
        app.push_line(
            ActiveTarget::Xuannv,
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "hi".into(),
            },
        );
        let backend = TestBackend::new(80, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| app.draw(f)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let all: String = (0..buf.area.height)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(!compact.contains("任务"), "默认应无 roster 标题:\n{all}");
        assert!(compact.contains("玄女"), "对话区玄女字样必在:\n{all}");
    }

    #[test]
    fn f_key_toggles_roster_overlay() {
        let mut app = ReplApp::stub();
        assert!(!app.roster_overlay_open, "初始 roster overlay 应关");
        app.handle_key(KeyCode::F(4), KeyModifiers::empty());
        assert!(app.roster_overlay_open, "F4 后 roster overlay 应开");
        assert_eq!(app.focus, Focus::Roster, "开 roster overlay 应顺带切焦点");

        app.handle_key(KeyCode::F(4), KeyModifiers::empty());
        assert!(!app.roster_overlay_open, "再按 F4 应关");
        assert_eq!(app.focus, Focus::Input, "关掉后焦点回到输入");
    }

    #[test]
    fn f_key_toggles_meta_overlay() {
        let mut app = ReplApp::stub();
        assert!(!app.meta_overlay_open, "初始 meta overlay 应关");
        app.handle_key(KeyCode::F(5), KeyModifiers::empty());
        assert!(app.meta_overlay_open, "F5 后 meta overlay 应开");
        app.handle_key(KeyCode::F(5), KeyModifiers::empty());
        assert!(!app.meta_overlay_open, "再按 F5 应关");
    }

    #[test]
    fn f4_and_f5_are_mutually_exclusive() {
        // 同时只能开一个 overlay——F5 时 F4 已开的应被覆盖关掉。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::F(4), KeyModifiers::empty());
        assert!(app.roster_overlay_open);
        app.handle_key(KeyCode::F(5), KeyModifiers::empty());
        assert!(app.meta_overlay_open);
        assert!(!app.roster_overlay_open, "F5 打开 meta 时 roster 应自动关");
    }

    // ───────── #17 R-popup-integ · SlashPopup 接入 repl ─────────

    #[test]
    fn slash_opens_popup_when_input_empty() {
        let mut app = ReplApp::stub();
        assert!(!app.popup.is_open(), "初始 popup 关");
        assert!(app.input_text().is_empty(), "前提：输入为空");

        // '/' 作为第一个字符应当开 popup，且不 insert 到 textarea。
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        assert!(app.popup.is_open(), "空输入 + / 应开 popup");
        assert!(
            app.input_text().is_empty(),
            "/ 不应 insert 到 textarea：实际 {:?}",
            app.input_text()
        );
    }

    #[test]
    fn typing_slash_mid_text_does_not_open_popup() {
        let mut app = ReplApp::stub();
        // 先往 textarea 敲两个非 / 字符。
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        assert_eq!(app.input_text(), "hi");
        assert!(!app.popup.is_open());

        // 句中按 / 不触发 popup，应当作普通字符输入给 textarea。
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        assert!(!app.popup.is_open(), "句中 / 不该开 popup");
        assert_eq!(app.input_text(), "hi/", "/ 应作为普通字符入输入");
    }

    #[test]
    fn popup_key_pass_through() {
        // popup 开着时所有键都交给 popup 而非 textarea / 全局。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        assert!(app.popup.is_open());

        // 键入 'h' → filter 变 "h"，候选收缩到 /help；textarea 不该被动。
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        assert!(app.popup.is_open(), "popup 保持 open");
        assert!(app.input_text().is_empty(), "textarea 不该收到 h");
        assert_eq!(app.popup.display_input(), "/h");
        assert_eq!(app.popup.candidates().len(), 1);
        assert_eq!(app.popup.candidates()[0].slash, "/help");
    }

    #[test]
    fn popup_execute_routes_to_action() {
        // 从打开 popup → 键入过滤 → Enter → Action 被 run_command_action 路由。
        // 以 /help 为验证入口：run 后玄女 bucket 里应被推入 help markdown。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty()); // /h → /help
        assert_eq!(app.popup.candidates()[0].slash, "/help");

        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, None, "popup 的 Enter 返 None，不走 Submit 路径");
        assert!(!app.popup.is_open(), "Execute 后 popup 自闭合");

        let bucket = app
            .dialogues
            .get(&ActiveTarget::Xuannv)
            .expect("玄女 bucket 应被创建");
        let all: String = bucket
            .iter()
            .filter_map(|e| match &e.line {
                DialogueLine::System(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("伏羲命令"), "应出 /help 内容：\n{all}");
        assert!(all.contains("/theme"), "应列 /theme：\n{all}");
    }

    #[test]
    fn popup_esc_closes_first_not_overlay_or_interrupt() {
        // 优先级：popup > overlay > interrupt。
        // popup 和 overlay 同时开时，按 Esc 只关 popup，overlay 保留。
        let mut app = ReplApp::stub();
        app.roster_overlay_open = true;
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        assert!(app.popup.is_open());
        assert!(app.roster_overlay_open);

        app.handle_key(KeyCode::Esc, KeyModifiers::empty());
        assert!(!app.popup.is_open(), "Esc 应先关 popup");
        assert!(app.roster_overlay_open, "overlay 保留");
        assert_eq!(app.esc_count, 0, "不该动 esc 双击计数");
    }

    #[test]
    fn popup_executes_theme_action_from_registry() {
        // /theme 无参经 popup → run_command_action 该触发 execute_theme_command(None)
        // → toast 列出可选主题（含 mocha / latte）。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        // /h, /t, /k, /c, /q, /s - /t 是 theme 独唯一 t 开头。
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty());
        assert_eq!(app.popup.candidates()[0].slash, "/theme");

        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert!(!app.popup.is_open());
        // 默认 action 是 Theme(None) → Info toast 列可选主题。
        let has_info = app.toasts.iter().any(|t| {
            t.variant == crate::toast::ToastVariant::Info
                && (t.text.contains("mocha") || t.text.contains("latte"))
        });
        assert!(
            has_info,
            "应有 Info toast 含可选主题：{:?}",
            app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn popup_quit_action_sets_should_quit() {
        let mut app = ReplApp::stub();
        assert!(!app.should_quit);
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('q'), KeyModifiers::empty()); // /q → /quit
        assert_eq!(app.popup.candidates()[0].slash, "/quit");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert!(app.should_quit, "/quit 该设 should_quit");
    }

    #[test]
    fn popup_clear_action_empties_active_bucket() {
        let mut app = ReplApp::stub();
        app.push_line(
            ActiveTarget::Xuannv,
            DialogueLine::Agent {
                name: "玄女".into(),
                text: "noise".into(),
            },
        );
        assert!(!app.dialogues.get(&ActiveTarget::Xuannv).unwrap().is_empty());

        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('c'), KeyModifiers::empty()); // /c → /clear
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert!(
            app.dialogues.get(&ActiveTarget::Xuannv).unwrap().is_empty(),
            "/clear 该清掉当前 active 的 bucket"
        );
    }

    // ───────── R10-integ /theme submit 接线 ─────────

    #[test]
    fn try_handle_slash_theme_with_arg_switches_and_toasts_success() {
        let _g = crate::theme::tests::current_theme_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::theme::set_theme("mocha").ok();

        let mut app = ReplApp::stub();
        let took = app.try_handle_slash_submit("/theme latte");
        assert!(took, "/theme 带名应被本地 handler 吃掉");
        assert_eq!(
            crate::theme::current(),
            crate::theme::Theme::catppuccin_latte(),
            "CURRENT 应切到 latte"
        );
        // 有 Success toast。
        assert!(
            app.toasts
                .iter()
                .any(|t| t.text.contains("latte")
                    && t.variant == crate::toast::ToastVariant::Success),
            "期待 Success toast 含 latte：{:?}",
            app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
        );
        crate::theme::set_theme("mocha").ok();
    }

    #[test]
    fn try_handle_slash_theme_unknown_toasts_error() {
        let _g = crate::theme::tests::current_theme_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::theme::set_theme("mocha").ok();

        let mut app = ReplApp::stub();
        let before = crate::theme::current();
        let took = app.try_handle_slash_submit("/theme nosuch-zzz");
        assert!(took, "/theme 未知名仍被 handler 吃掉（报错而非派给 agent）");
        assert_eq!(crate::theme::current(), before, "失败不该改 CURRENT");
        assert!(
            app.toasts
                .iter()
                .any(|t| t.variant == crate::toast::ToastVariant::Error),
            "期待 Error toast"
        );
    }

    #[test]
    fn try_handle_slash_theme_no_arg_lists_available_via_info_toast() {
        let mut app = ReplApp::stub();
        app.execute_theme_command(None);
        let info_texts: Vec<&str> = app
            .toasts
            .iter()
            .filter(|t| t.variant == crate::toast::ToastVariant::Info)
            .map(|t| t.text.as_str())
            .collect();
        assert!(
            info_texts.iter().any(|t| t.contains("mocha")),
            "无参 /theme 的 Info toast 应含 mocha：{:?}",
            info_texts
        );
        assert!(
            info_texts.iter().any(|t| t.contains("latte")),
            "无参 /theme 的 Info toast 应含 latte：{:?}",
            info_texts
        );
    }

    // ───────── R11 /help submit 接线 ─────────

    #[test]
    fn try_handle_slash_help_dumps_system_lines_to_active_bucket() {
        let mut app = ReplApp::stub();
        let took = app.try_handle_slash_submit("/help");
        assert!(took, "/help 应被本地 handler 吃掉");

        let bucket = app
            .dialogues
            .get(&ActiveTarget::Xuannv)
            .expect("玄女 bucket 应被创建");
        // 起点 bucket 为空，/help 后应有多条 System 行。
        assert!(
            !bucket.is_empty(),
            "/help 后 bucket 应有 System 行：{bucket:?}"
        );
        // 首行应是 "# 伏羲命令" 标题。
        let first = bucket.front().expect("至少一条");
        match &first.line {
            DialogueLine::System(s) => {
                assert!(s.contains("伏羲命令"), "首行应含标题：{s}")
            }
            other => panic!("首行应是 System，实际 {other:?}"),
        }
        // 校验命令名也写进去了。
        let all: String = bucket
            .iter()
            .filter_map(|e| match &e.line {
                DialogueLine::System(s) => Some(s.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("/help"), "应含 /help：\n{all}");
        assert!(all.contains("/theme"), "应含 /theme：\n{all}");
    }

    #[test]
    fn try_handle_slash_submit_rejects_non_slash() {
        let mut app = ReplApp::stub();
        assert!(!app.try_handle_slash_submit("hello"));
        assert!(
            !app.try_handle_slash_submit("  /theme"),
            "前导空白不算 slash"
        );
    }

    #[test]
    fn esc_closes_overlay_first_before_other_actions() {
        // overlay 打开时按 Esc 只关 overlay，不走双击 Esc 中断 / 回玄女 的旧路径。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::F(4), KeyModifiers::empty());
        assert!(app.roster_overlay_open);
        // 伪造一个 xuannv 以外的 active——若 Esc 误走 reset_to_xuannv 会让 active 变。
        // 这里 active 本就是 Xuannv，用 esc_count 观察：Esc 应保持 count=0。
        app.esc_count = 0;
        app.esc_last_at = None;

        app.handle_key(KeyCode::Esc, KeyModifiers::empty());
        assert!(!app.roster_overlay_open, "Esc 应先关 overlay");
        assert_eq!(app.esc_count, 0, "Esc 关 overlay 时不该动 esc_count");
        assert!(app.esc_last_at.is_none(), "esc_last_at 不该被设置");
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

    /// D13：user-turn title 的 task 节点用 `·` + muted。
    #[test]
    fn user_turn_task_uses_dim_dot() {
        let (icon, color) = task_icon_and_color("user-turn", false);
        assert_eq!(icon, "· ");
        assert_eq!(color, theme().muted());
    }

    /// D13：正经 task 用 `◇` + White；prune 后降 muted。
    #[test]
    fn regular_task_uses_diamond() {
        let (icon, color) = task_icon_and_color("修 bug", false);
        assert_eq!(icon, "◇ ");
        assert_eq!(color, Color::White);

        let (icon2, color2) = task_icon_and_color("修 bug", true);
        assert_eq!(icon2, "◇ ", "prune 不变 icon");
        assert_eq!(color2, theme().muted(), "prune 后 muted");
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

    /// M3.5 回归：`Args::default()` 里 `"127.0.0.1:4100".parse().expect(...)`
    /// 是仅有的 production-code expect。这条测试钉死它的 SAFETY 不变式，
    /// 防止未来有人改字面量没改 expect 把 Default 弄崩。
    #[test]
    fn args_default_does_not_panic_and_binds_to_loopback() {
        let args = Args::default();
        assert_eq!(args.bind.port(), 4100);
        assert!(
            args.bind.ip().is_loopback(),
            "默认必须 loopback，避免误暴露"
        );
        assert_eq!(args.xuannv_role, "xuannv");
    }
}
