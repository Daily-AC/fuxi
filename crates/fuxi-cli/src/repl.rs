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
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tui_textarea::{Input, TextArea};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 每个对话桶最多保留多少行。
const DIALOGUE_CAP: usize = 500;
/// 每秒刷 UI 的键盘 poll 窗口。
const KEY_POLL: Duration = Duration::from_millis(50);
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
    /// 分布式 controller token（worker/enqueue 都要带）；不填则读 `$FUXI_DIST_TOKEN`。
    #[arg(long = "dist-token")]
    pub dist_token: Option<String>,
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
            dist_token: None,
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
/// - 默认不打印（首屏走极简，避免装饰噪声）
/// - `FUXI_BANNER=on` / `1` / `true`（任意大小写）→ 打印
/// - stdout 非 tty（被管道 / 重定向）→ 跳过（避免污染脚本输出）
fn should_show_banner() -> bool {
    use std::io::IsTerminal;
    let enabled = std::env::var("FUXI_BANNER")
        .ok()
        .map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("on") || v.eq_ignore_ascii_case("true") || v == "1"
        })
        .unwrap_or(false);
    enabled && std::io::stdout().is_terminal()
}

pub async fn run(args: Args) -> Result<()> {
    require_fuxi_in_path("fuxi", std::env::var_os("PATH").as_deref())?;

    // D17 · 启动 banner（现改为默认关闭）：首屏追求极简，不默认塞装饰块。
    // 仅当 FUXI_BANNER=on/true/1 且 stdout 是 tty 时打印。
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
    let dist_token = args
        .dist_token
        .clone()
        .or_else(|| std::env::var(crate::dist::DIST_TOKEN_ENV).ok());
    let (app_router, dist_ctrl) = if let Some(token) = dist_token {
        let dist_ctrl = Arc::new(crate::dist::DistController::new(token, bus.clone()));
        crate::dist::spawn_sweep_task(dist_ctrl.clone());
        // path 3 α: 同 up.rs——有 HMAC secret env 走鉴权 router；缺则 warn + 老版无鉴权
        let dist_router_built = match crate::dist_auth::HmacSecret::from_env() {
            Ok(secret) => {
                let gate = crate::dist_auth::HmacGate::new(secret);
                crate::dist::router_with_hmac(dist_ctrl.clone(), gate)
            }
            Err(reason) => {
                tracing::warn!(
                    %reason,
                    "FUXI_DIST_HMAC_SECRET 未设置，/dist/* 暂以无鉴权方式运行——生产部署务必配置"
                );
                crate::dist::router(dist_ctrl.clone())
            }
        };
        let router = app_router.merge(dist_router_built);
        (router, Some(dist_ctrl))
    } else {
        (app_router, None)
    };
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

    let outcome = drive_tui(bus, fuxi.clone(), xuannv_id, dist_ctrl, resume_banner).await;

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

/// 远端 worker 节点状态——拓扑面板用。
/// `WorkerHeartbeatStateChanged.status` 字符串 `"alive"`/`"stale"` 解码后的内部表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NodeStatus {
    Alive,
    Stale,
    /// 未知状态——只有 WorkerRegistered 触达过、还没收到心跳时的过渡态。
    /// register 是"声明能力"，inflight/health 由后续心跳决定。
    Unknown,
}

/// 远端 worker 节点的本地视图——TUI 拓扑面板状态机的最小单元。
///
/// 字段集**只**反映 γ EventKind 能携带的信息（+ 入栈时刻）；α 的 `NodeSnapshot`
/// 是 wire 类型，初始 snapshot 灌入时通过 `apply_snapshot()` 转换，避免 TUI 直接
/// 依赖 IPC schema。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeView {
    pub node_id: String,
    pub status: NodeStatus,
    pub tags: Vec<String>,
    pub inflight: u32,
    pub max_concurrency: u32,
    /// 最近一次本地接收事件的时刻——LAST_SEEN 列展示用，相对 now() 计算。
    pub last_event_at: Instant,
    /// 最近一次 sweep 事件回收的 job 数——状态栏/选中行展示参考。
    /// 0 不代表"从未 sweep"——sweep 后死活并存。
    pub last_recycled_count: u32,
}

/// 主对话对象——对谁说话、右栏展示谁。
///
/// `pub`：被 `draft_stash::DraftStash` 的 pub 方法签名引用，pub(crate) 会
/// 触发 `private_interfaces` warning（lib 化后所有 mod 都 pub 暴露）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveTarget {
    Xuannv,
    Worker(AgentId),
}

/// 一条对话行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DialogueLine {
    User(String),
    /// agent 自称——玄女 / 门客都用这种。
    Agent {
        name: String,
        text: String,
    },
    Tool {
        text: String,
        ok: bool,
    },
    #[allow(dead_code)]
    System(String),
}

/// 对话区条目 = 时间戳 + 消息。
///
/// 时间戳当前不在默认 UI 展示（采用 cc 风格简前缀），但保留字段便于
/// 后续切换显示策略（例如 debug/审计视图）时直接复用。
#[derive(Debug, Clone)]
pub(crate) struct DialogueEntry {
    #[allow(dead_code)] // 当前样式不显式展示时间，保留字段便于未来切换显示策略。
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

/// 任务节点——左栏任务树的基本单位。
#[derive(Debug, Clone)]
pub(crate) struct TaskNode {
    pub task_id: TaskId,
    pub title: String,
    pub description: String,
    pub state: TaskState,
    pub worker: AgentId,
    pub worker_role: String,
    /// 任务首次派发时刻。Done 后保留用于审计，不再驱动 TTL 清理。
    pub dispatched_at: Instant,
    pub thinking: bool,
    pub worktree: Option<PathBuf>,
    /// 最近工具调用摘要 `tool=args前40字`，右栏展示。
    pub recent_tools: VecDeque<String>,
}

/// 左栏扁平行——用于渲染 + `roster_state` 选中计算。
#[derive(Debug, Clone, Copy)]
pub(crate) enum PaneRow {
    GroupHeader(usize),
    Task(usize),
}

/// 用户按 Enter 后计算出的提交意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Submit {
    Xuannv(String),
    Worker(AgentId, String),
    Kill(AgentId),
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
    /// 角色真相源（agent_id -> role）。用于 task-bound 过渡期去除对 idle 桶的语义依赖。
    pub(crate) roles_by_agent: HashMap<AgentId, String>,

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
    pub(crate) ctrl_c_last_at: Option<Instant>,
    pub(crate) ctrl_c_count: u8,

    /// 鼠标点击区注册表。每帧 `draw()` 开头 `clear()`，各 draw_*
    /// 末尾 `register(area, ClickAction::Xxx)`。mouse 事件 hit_test 分派。
    pub(crate) click: ClickRegistry<ClickAction>,

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
    /// spinner 变速节流计数：避免每帧跳动造成焦虑感。
    pub(crate) spinner_tick_gate: u8,

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
    /// 是否真的发生过拖拽（Down 仅定位，Drag 才算选区）。
    pub(crate) selection_dragged: bool,
    /// 最近一次 draw_dialogue 的区域——鼠标坐标 → 对话行索引需要它。
    pub(crate) last_dialogue_area: Option<Rect>,
    /// 最近一次 draw_dialogue 产出的「按屏宽展开后的可见文本行」。
    /// 用于拖选复制时按区域截取，避免“整行复制”体验。
    pub(crate) last_dialogue_wrapped_rows: Vec<String>,

    /// roster（任务 / 门客列表）overlay 开关。
    /// WHY overlay 而非常驻左栏：单栏主体让对话区拉满宽度（方案 R9），roster
    /// 平时收起来不干扰；用户 F4 临时看一眼就好。Esc 优先关 overlay。
    pub(crate) roster_overlay_open: bool,
    /// meta（active target 的元信息）overlay 开关。同上，F5 切。
    pub(crate) meta_overlay_open: bool,
    /// help（命令说明）overlay 开关。`/help` 打开，Esc 关闭。
    pub(crate) help_overlay_open: bool,
    /// /nodes 拓扑 overlay 开关。F6 切，`/nodes` 打开。
    pub(crate) nodes_overlay_open: bool,
    /// 远端 worker 视图——live update 由 EventBus WorkerRegistered/HeartbeatStateChanged/StaleSwept
    /// 推送增量；初始 snapshot 由 IPC `Command::Nodes` 一次性灌（α 实装后）。
    /// WHY 不用 HashMap：节点数 O(10)，Vec 顺序展示稳定（按 node_id 排序）。
    pub(crate) nodes: Vec<NodeView>,
    /// 拓扑 overlay 选中行游标，超出 nodes.len() 时 draw 时 clamp。
    pub(crate) nodes_selected: usize,
    /// /tree 配置：true=左侧常驻任务树；false=单栏 + 按需浮层。
    pub(crate) tree_sidebar_enabled: bool,
    /// 任务树折叠状态：key 为 task_id 字符串（稳定，不受同名任务影响）。
    pub(crate) collapsed_task_groups: HashSet<String>,

    /// 斜杠命令浮层（#17 接入 #13 的 SlashPopup）。
    /// WHY 放 ReplApp：popup 有自己的状态（open/filter/selected），要跨多帧持久。
    pub(crate) popup: crate::autocomplete::SlashPopup,
    /// 命令注册表——popup 的候选源 + slash submit 的 action 源。
    /// 每次用完都调 `register_default()` 太浪费（R11 /help 测过也没事，但整合后
    /// popup 每次 filter 都要它，应存一份）。后续 /theme 插件想增删命令时改这个。
    pub(crate) cmd_registry: crate::command_registry::CommandRegistry,
    /// TeammateSpinnerTree 帧计数（每帧自增，用于每门客 spinner 动画）。
    pub(crate) teammate_tree_tick: u64,
    /// slash action 产生的异步提交（如 /kill）由 handle_key 末尾取走返回给 drive_tui。
    pub(crate) pending_submit: Option<Submit>,
    /// 输入区显示 `[image #n]`，发送前再按索引展开成真实路径。
    pub(crate) image_attachments: HashMap<usize, PathBuf>,
    /// 每个 agent 最近一次 tool started 的可读名（给 tooluse_xxx finished 做回填）。
    pub(crate) last_tool_label_by_agent: HashMap<AgentId, String>,
}

/// 双击 Esc 的判定窗口。2s 太紧会让真想中断的用户按不上；太松会跟单击混。
pub(crate) const ESC_DOUBLE_WINDOW: Duration = Duration::from_secs(2);
pub(crate) const CTRL_C_DOUBLE_WINDOW: Duration = Duration::from_secs(2);

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            s == "1" || s == "true" || s == "yes" || s == "on"
        })
        .unwrap_or(false)
}

fn env_true_by_default(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let s = v.trim().to_ascii_lowercase();
            !(s == "0" || s == "false" || s == "no" || s == "off")
        }
        Err(_) => true,
    }
}

fn has_os_shortcut_modifier(mods: KeyModifiers) -> bool {
    mods.contains(KeyModifiers::SUPER)
        || mods.contains(KeyModifiers::META)
        || mods.contains(KeyModifiers::HYPER)
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| {
            matches!(
                s.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "tiff"
                    | "tif"
                    | "heic"
                    | "heif"
                    | "svg"
            )
        })
        .unwrap_or(false)
}

fn split_shell_like_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn normalize_path_token(tok: &str) -> Option<PathBuf> {
    let t = tok.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with("file://")
        && let Ok(url) = url::Url::parse(t)
        && let Ok(p) = url.to_file_path()
    {
        return Some(p);
    }
    Some(PathBuf::from(t))
}

fn default_attachment_dir() -> PathBuf {
    std::env::var_os("FUXI_ATTACHMENT_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|d| d.join(".fuxi/attachments"))
        })
        .unwrap_or_else(|| PathBuf::from(".fuxi/attachments"))
}

fn new_textarea() -> TextArea<'static> {
    let mut ta = TextArea::default();
    ta.set_cursor_line_style(Style::default());
    ta.set_placeholder_text("");
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
            roles_by_agent: HashMap::new(),
            roster_state: ListState::default(),
            events_visible: false,
            events: FirehoseApp::new(),
            input: new_textarea(),
            dialogue_scroll: 0,
            dialogue_auto_scroll: true,
            last_dialogue_total: 0,
            last_dialogue_view: 0,
            should_quit: false,
            ctrl_c_last_at: None,
            ctrl_c_count: 0,
            click: ClickRegistry::new(),
            esc_last_at: None,
            esc_count: 0,
            toasts: crate::toast::ToastStack::new(),
            status_spinner: crate::spinner::Spinner::new(),
            spinner_tick_gate: 0,
            xuannv_busy_since: None,
            history: crate::prompt_history::PromptHistory::default(),
            stash: crate::draft_stash::DraftStash::new(),
            selection_anchor: None,
            selection_cursor: None,
            selection_dragged: false,
            last_dialogue_area: None,
            last_dialogue_wrapped_rows: Vec::new(),
            roster_overlay_open: false,
            meta_overlay_open: false,
            help_overlay_open: false,
            nodes_overlay_open: false,
            nodes: Vec::new(),
            nodes_selected: 0,
            tree_sidebar_enabled: env_truthy("FUXI_TREE_SIDEBAR"),
            collapsed_task_groups: HashSet::new(),
            popup: crate::autocomplete::SlashPopup::new(),
            cmd_registry: crate::command_registry::register_default(),
            teammate_tree_tick: 0,
            pending_submit: None,
            image_attachments: HashMap::new(),
            last_tool_label_by_agent: HashMap::new(),
        };
        app.roster_state.select(Some(0));
        app
    }

    /// 构造仅含玄女的 app——测试帮手。
    #[cfg(test)]
    fn stub() -> Self {
        let mut app = Self::new(AgentId::new());
        app.tree_sidebar_enabled = false;
        app
    }

    /// 当前输入文本（`textarea.lines()` 按换行拼回）。
    pub(crate) fn input_text(&self) -> String {
        self.input.lines().join("\n")
    }

    /// 粘贴事件：
    /// - 若像"文件拖放/路径粘贴"（全部 token 都是存在的文件）→ 转成附件引用
    /// - 否则按普通文本插入 textarea
    ///
    /// 公理：bracketed paste 让 IME / 剪贴板整块内容一次进入，避免逐键 race。
    pub(crate) fn handle_paste(&mut self, s: &str) {
        self.focus = Focus::Input;
        if self.try_insert_pasted_files(s) {
            return;
        }
        self.input.insert_str(s);
    }

    fn next_image_index(&self) -> usize {
        let mut max_idx = 0usize;
        let mut rest = self.input_text();
        while let Some(pos) = rest.find("[image #") {
            let tail = &rest[pos + "[image #".len()..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<usize>() {
                max_idx = max_idx.max(n);
            }
            rest = tail.to_string();
        }
        for idx in self.image_attachments.keys() {
            max_idx = max_idx.max(*idx);
        }
        max_idx + 1
    }

    fn insert_attachment_refs(&mut self, refs: &[String]) {
        if refs.is_empty() {
            return;
        }
        if !self.input_text().trim().is_empty() {
            self.input.insert_newline();
        }
        self.input.insert_str(refs.join("\n"));
    }

    fn try_insert_pasted_files(&mut self, s: &str) -> bool {
        let raw = s.trim();
        if raw.is_empty() {
            return false;
        }
        let tokens = split_shell_like_tokens(raw);
        if tokens.is_empty() {
            return false;
        }
        let mut paths = Vec::with_capacity(tokens.len());
        for tok in tokens {
            let Some(p) = normalize_path_token(&tok) else {
                return false;
            };
            let abs = if p.is_absolute() {
                p
            } else if let Ok(cwd) = std::env::current_dir() {
                cwd.join(p)
            } else {
                return false;
            };
            if !abs.exists() || !abs.is_file() {
                return false;
            }
            let abs = abs.canonicalize().unwrap_or(abs);
            paths.push(abs);
        }
        if paths.is_empty() {
            return false;
        }

        let mut image_idx = self.next_image_index();
        let mut refs = Vec::with_capacity(paths.len());
        let mut image_count = 0usize;
        for p in &paths {
            if is_image_path(p) {
                refs.push(format!("[image #{image_idx}]"));
                self.image_attachments.insert(image_idx, p.clone());
                image_idx += 1;
                image_count += 1;
            } else {
                refs.push(p.display().to_string());
            }
        }
        self.insert_attachment_refs(&refs);
        self.toasts.push(
            format!(
                "已附加 {} 个文件{}",
                refs.len(),
                if image_count > 0 {
                    format!("（含 {image_count} 张图片）")
                } else {
                    String::new()
                }
            ),
            crate::toast::ToastVariant::Success,
            Duration::from_secs(3),
        );
        true
    }

    fn paste_from_system_clipboard(&mut self) {
        match crate::clipboard::read_text_from_clipboard() {
            Ok(Some(s)) if !s.trim().is_empty() => self.handle_paste(&s),
            Ok(Some(_)) => {
                if !self.try_paste_image_from_clipboard() {
                    self.toasts.push(
                        "剪贴板里没有可粘贴文本/图片",
                        crate::toast::ToastVariant::Info,
                        Duration::from_secs(3),
                    );
                }
            }
            Ok(None) => {
                if !self.try_paste_image_from_clipboard() {
                    self.toasts.push(
                        "当前平台不支持读取系统剪贴板",
                        crate::toast::ToastVariant::Error,
                        Duration::from_secs(3),
                    );
                }
            }
            Err(e) => self.toasts.push(
                format!("读取剪贴板失败：{e}"),
                crate::toast::ToastVariant::Error,
                Duration::from_secs(3),
            ),
        }
    }

    fn try_paste_image_from_clipboard(&mut self) -> bool {
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;
            let dir = default_attachment_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                self.toasts.push(
                    format!("创建附件目录失败：{e}"),
                    crate::toast::ToastVariant::Error,
                    Duration::from_secs(3),
                );
                return false;
            }
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let file = dir.join(format!("clipboard-{ts}.png"));
            let ok = Command::new("pngpaste")
                .arg(&file)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok || !file.exists() {
                return false;
            }
            let abs = file.canonicalize().unwrap_or(file);
            let idx = self.next_image_index();
            self.image_attachments.insert(idx, abs);
            self.insert_attachment_refs(&[format!("[image #{idx}]")]);
            self.toasts.push(
                "已粘贴剪贴板图片",
                crate::toast::ToastVariant::Success,
                Duration::from_secs(3),
            );
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub(crate) fn push_line(&mut self, target: ActiveTarget, line: DialogueLine) {
        self.push_entry(target, DialogueEntry::new(line));
    }

    fn push_entry(&mut self, target: ActiveTarget, entry: DialogueEntry) {
        let bucket = self.dialogues.entry(target).or_default();
        // 折叠连续完全重复消息，避免 API 限流等错误刷屏。
        if let Some(last) = bucket.back()
            && last.line == entry.line
        {
            return;
        }
        if bucket.len() == DIALOGUE_CAP {
            bucket.pop_front();
        }
        bucket.push_back(entry);
    }

    fn expand_image_refs_for_submit(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 32);
        let mut rest = text;
        loop {
            let Some(pos) = rest.find("[image #") else {
                out.push_str(rest);
                break;
            };
            out.push_str(&rest[..pos]);
            let tail = &rest[pos + "[image #".len()..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                out.push_str("[image #");
                rest = tail;
                continue;
            }
            let after_digits = &tail[digits.len()..];
            if let Some(rest_after_bracket) = after_digits.strip_prefix(']') {
                if let Ok(idx) = digits.parse::<usize>()
                    && let Some(path) = self.image_attachments.get(&idx)
                {
                    out.push_str(&format!("[image #{idx}] {}", path.display()));
                    rest = rest_after_bracket;
                    continue;
                }
                out.push_str(&format!("[image #{digits}]"));
                rest = rest_after_bracket;
                continue;
            }
            out.push_str("[image #");
            out.push_str(&digits);
            rest = after_digits;
        }
        out
    }

    fn display_task_title(raw: &str) -> String {
        let t = raw.trim();
        if t.is_empty() {
            "任务".to_string()
        } else {
            t.to_string()
        }
    }

    fn is_hidden_tree_task(title: &str) -> bool {
        let normalized = title.trim().to_ascii_lowercase();
        normalized == "user-turn" || normalized.starts_with("user-turn ")
    }

    fn is_xuannv_role(role: &str) -> bool {
        role.trim().eq_ignore_ascii_case("xuannv") || role.trim() == "玄女"
    }

    fn visible_task_groups(&self) -> Vec<(String, String, Vec<usize>)> {
        let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
        for (idx, task) in self.tasks.iter().enumerate() {
            if task.worker == self.xuannv_id || Self::is_xuannv_role(&task.worker_role) {
                continue;
            }
            let title = Self::display_task_title(&task.title);
            if Self::is_hidden_tree_task(&title) {
                continue;
            }
            // task-rooted：按 task_id 聚类，禁止同标题不同 task 被错误合并。
            let key = task.task_id.to_string();
            if let Some((_, _, members)) = groups.iter_mut().find(|(k, _, _)| *k == key) {
                members.push(idx);
            } else {
                groups.push((key, title, vec![idx]));
            }
        }
        groups
    }

    /// 左栏扁平行——父任务（可折叠骨架）+ 子门客行。
    pub(crate) fn pane_rows(&self) -> Vec<PaneRow> {
        let groups = self.visible_task_groups();
        let mut rows = Vec::new();
        for (gidx, (key, _, members)) in groups.iter().enumerate() {
            rows.push(PaneRow::GroupHeader(gidx));
            if self.collapsed_task_groups.contains(key) {
                continue;
            }
            for m in members {
                rows.push(PaneRow::Task(*m));
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
                    self.roles_by_agent.insert(id, role.clone());
                    if id == self.xuannv_id {
                        self.xuannv_status = ShelfStatus::Idle;
                        self.refresh_xuannv_busy_anchor();
                    }
                }
            }
            EventKind::AgentReady { .. } => {
                if let Some(id) = who
                    && id == self.xuannv_id
                {
                    self.xuannv_status = ShelfStatus::Idle;
                    self.refresh_xuannv_busy_anchor();
                }
            }
            EventKind::AgentDead { cause } => {
                if let Some(id) = who {
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
            EventKind::ToolCallFinished {
                tool,
                ok,
                output_preview,
            } => {
                if let Some(id) = who {
                    let target = tgt(xuannv, id);
                    let preview = truncate_by_width(output_preview, 96);
                    let label = self
                        .last_tool_label_by_agent
                        .get(&id)
                        .filter(|_| tool.starts_with("tooluse_"))
                        .cloned()
                        .unwrap_or_else(|| humanize_tool_name(tool));
                    let text = if preview.trim().is_empty() {
                        label
                    } else {
                        format!("{} · {}", label, summarize_tool_preview(&preview))
                    };
                    self.push_line(target, DialogueLine::Tool { text, ok: *ok });
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
                    self.roles_by_agent.insert(*to, role.clone());
                    self.upsert_task(tid, *to, role);
                }
            }
            EventKind::TaskCreated { title, description } => {
                if let (Some(id), Some(tid)) = (who, ev.meta.task) {
                    if id != self.xuannv_id {
                        let role = self.lookup_role(id);
                        self.roles_by_agent.insert(id, role.clone());
                        self.upsert_task(tid, id, role);
                    }
                    for t in self.tasks.iter_mut().filter(|t| t.task_id == tid) {
                        t.title = title.clone();
                        t.description = description.clone();
                    }
                }
            }
            EventKind::TaskStateChanged { to, .. } => {
                if let Some(tid) = ev.meta.task {
                    if matches!(to, TaskState::Done | TaskState::Cancelled)
                        && let Some(done_agent) = who
                        && done_agent != self.xuannv_id
                    {
                        let role = self.role_display(&self.lookup_role(done_agent));
                        let title = self
                            .tasks
                            .iter()
                            .find(|t| t.task_id == tid && t.worker == done_agent)
                            .map(|t| Self::display_task_title(&t.title))
                            .unwrap_or_else(|| "任务".to_string());
                        let done_verb = if matches!(to, TaskState::Done) {
                            "已完成"
                        } else {
                            "已取消"
                        };
                        self.push_line(
                            ActiveTarget::Xuannv,
                            DialogueLine::System(format!("{role} {done_verb}：{title}")),
                        );
                    }
                    let target_agent = who;
                    let mut matched = false;
                    for t in self.tasks.iter_mut().filter(|t| {
                        t.task_id == tid && target_agent.is_some_and(|aid| t.worker == aid)
                    }) {
                        t.state = *to;
                        matched = true;
                    }
                    if matched {
                        return;
                    }
                    for t in self.tasks.iter_mut().filter(|t| t.task_id == tid) {
                        t.state = *to;
                    }
                }
            }
            // WHY 删除 TaskDelivered/TaskCancelled 分支（M3.6）：
            // 这俩孤儿变体已从 EventKind 移除——终态走上面 TaskStateChanged 分支
            // 中的 Done|Cancelled 已经在同一 task node 上更新状态。
            EventKind::ToolCallStarted { tool, args } => {
                if let Some(id) = who {
                    self.last_tool_label_by_agent
                        .insert(id, tool_arg_preview(tool, args));
                }
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
            // ── 分布式拓扑（P6）: live update 拓扑面板 ─────────────
            // WHY 不要轮询：公理 3——TUI 拓扑视图全靠订阅这三个事件做增量。
            EventKind::WorkerRegistered {
                node_id,
                tags,
                max_concurrency,
            } => {
                self.upsert_node_on_register(node_id, tags.clone(), *max_concurrency);
            }
            EventKind::WorkerHeartbeatStateChanged {
                node_id,
                inflight_count,
                status,
            } => {
                self.apply_node_heartbeat(node_id, *inflight_count, *status);
            }
            EventKind::WorkerStaleSwept {
                node_id,
                recycled_jobs,
            } => {
                self.mark_node_stale(node_id, recycled_jobs.len() as u32);
            }
            _ => {}
        }
    }

    /// `WorkerRegistered` 落地：节点首达就插入；重连只更新 tags + max_concurrency，
    /// 保留 inflight/status——register 是"声明能力"不是"清状态"，沿 dist.rs 注释。
    pub(crate) fn upsert_node_on_register(
        &mut self,
        node_id: &str,
        tags: Vec<String>,
        max_concurrency: u32,
    ) {
        let now = Instant::now();
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.tags = tags;
            node.max_concurrency = max_concurrency;
            node.last_event_at = now;
        } else {
            self.nodes.push(NodeView {
                node_id: node_id.to_string(),
                status: NodeStatus::Unknown,
                tags,
                inflight: 0,
                max_concurrency,
                last_event_at: now,
                last_recycled_count: 0,
            });
            self.nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        }
    }

    /// `WorkerHeartbeatStateChanged` 落地：找到节点更 inflight + status，
    /// `status` 字符串只识别 `"alive"`/`"stale"`，其他归 Unknown。
    /// 找不到节点不创建——心跳不应早于 register。
    pub(crate) fn apply_node_heartbeat(
        &mut self,
        node_id: &str,
        inflight_count: u32,
        status: fuxi_core::WorkerStatus,
    ) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.inflight = inflight_count;
            node.status = node_status_from_enum(status);
            node.last_event_at = Instant::now();
        }
    }

    /// `WorkerStaleSwept` 落地：标 stale + 记 recycled 数。inflight 保留——
    /// sweep 只搬 job 不改 worker 的 inflight 字段（dist.rs sweep_stale 行为）。
    pub(crate) fn mark_node_stale(&mut self, node_id: &str, recycled: u32) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.status = NodeStatus::Stale;
            node.last_recycled_count = recycled;
            node.last_event_at = Instant::now();
        }
    }

    /// 把 IPC 来的 NodeSnapshot 全表灌成 NodeView——开机 priming 用。
    /// 替换整个 self.nodes（不 merge，避免 stale 状态残留）。
    /// `last_seen_ms_ago` 转回 `Instant` 时用 `now - delta`，丢一点点精度但 TUI 显示不敏感。
    pub(crate) fn apply_snapshot(&mut self, snaps: Vec<crate::ipc::NodeSnapshot>) {
        let now = Instant::now();
        self.nodes = snaps
            .into_iter()
            .map(|s| {
                let last_event_at = s
                    .last_seen_ms_ago
                    .and_then(|ms| now.checked_sub(Duration::from_millis(ms)))
                    .unwrap_or(now);
                let status = decode_node_status(&s.status);
                NodeView {
                    node_id: s.node_id,
                    status,
                    tags: s.tags,
                    inflight: s.inflight_count as u32,
                    max_concurrency: s.max_concurrency,
                    last_event_at,
                    last_recycled_count: 0,
                }
            })
            .collect();
        self.nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    }

    /// 完成/取消后清理过期任务。由 drive_tui 每帧前调一次；测试里直接喂 Instant。
    pub(crate) fn tick(&mut self, now: Instant) {
        // Toast 到期 prune——draw 之前 prune 避免已死 toast 还闪一帧。
        self.toasts.prune(now);
    }

    fn handle_agent_dead(&mut self, id: AgentId) {
        if id == self.xuannv_id {
            self.xuannv_status = ShelfStatus::Dead;
            self.xuannv_thinking = false;
            self.xuannv_busy_since = None;
            return;
        }
        self.tasks.retain(|t| t.worker != id);
        self.roles_by_agent.remove(&id);
        if matches!(self.active, ActiveTarget::Worker(w) if w == id) {
            self.switch_active(ActiveTarget::Xuannv);
        }
        self.resync_roster_selection();
    }

    fn upsert_task(&mut self, task_id: TaskId, worker: AgentId, role: String) {
        if let Some(t) = self
            .tasks
            .iter_mut()
            .find(|t| t.task_id == task_id && t.worker == worker)
        {
            t.worker_role = role;
            // 重复派发同一 task_id 给同一门客时，不应重置 elapsed 计时。
            // 但若该门客此前处于终态，重新派活应重置起始时刻。
            if matches!(t.state, TaskState::Done | TaskState::Cancelled) {
                t.dispatched_at = Instant::now();
            }
            return;
        }
        if let Some(existing) = self.tasks.iter().find(|t| t.task_id == task_id).cloned() {
            let mut cloned = existing;
            cloned.worker = worker;
            cloned.worker_role = role;
            self.roles_by_agent
                .insert(worker, cloned.worker_role.clone());
            cloned.dispatched_at = Instant::now();
            cloned.thinking = false;
            cloned.recent_tools.clear();
            self.tasks.push(cloned);
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
            thinking: false,
            worktree: None,
            recent_tools: VecDeque::with_capacity(RECENT_TOOLS_CAP),
        };
        self.roles_by_agent.insert(worker, node.worker_role.clone());
        self.tasks.push(node);
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
            .filter(|t| t.worker == id)
            .max_by_key(|t| t.dispatched_at)
    }

    /// 取某 worker 当前最相关的 task id：优先最近一个未终态任务；若都终态，退到最近任务。
    fn latest_task_id_for_worker(&self, id: AgentId) -> Option<TaskId> {
        let pick = self
            .tasks
            .iter()
            .filter(|t| {
                t.worker == id && !matches!(t.state, TaskState::Done | TaskState::Cancelled)
            })
            .max_by_key(|t| t.dispatched_at)
            .or_else(|| {
                self.tasks
                    .iter()
                    .filter(|t| t.worker == id)
                    .max_by_key(|t| t.dispatched_at)
            })?;
        Some(pick.task_id)
    }

    fn lookup_role(&self, id: AgentId) -> String {
        if id == self.xuannv_id {
            return "xuannv".into();
        }
        if let Some(role) = self.roles_by_agent.get(&id) {
            return role.clone();
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
        self.role_display(&self.lookup_role(id))
    }

    fn role_display(&self, role: &str) -> String {
        if !role.is_ascii() {
            return role.to_string();
        }
        match role.to_ascii_lowercase().as_str() {
            "xuannv" => "玄女".to_string(),
            "luban" => "鲁班".to_string(),
            "zhudiesi" => "铸牒司".to_string(),
            "shaosiming" => "少司命".to_string(),
            "xiaoyi" => "小乙".to_string(),
            _ => role.to_string(),
        }
    }

    fn resync_roster_selection(&mut self) {
        let groups = self.visible_task_groups();
        let rows = self.pane_rows();
        let want = match self.active {
            ActiveTarget::Xuannv => None,
            ActiveTarget::Worker(id) => rows
                .iter()
                .position(|r| matches!(r, PaneRow::Task(i) if self.tasks[*i].worker == id))
                .or_else(|| {
                    let t_idx = self
                        .tasks
                        .iter()
                        .position(|t| t.worker == id && t.worker != self.xuannv_id)?;
                    let key = self.tasks[t_idx].task_id.to_string();
                    rows.iter().position(|r| match r {
                        PaneRow::GroupHeader(gidx) => groups
                            .get(*gidx)
                            .map(|(k, _, _)| k == &key)
                            .unwrap_or(false),
                        PaneRow::Task(_) => false,
                    })
                }),
        };
        let fallback = rows
            .iter()
            .position(|r| matches!(r, PaneRow::GroupHeader(_) | PaneRow::Task(_)))
            .or_else(|| (!rows.is_empty()).then_some(0));
        self.roster_state.select(want.or(fallback));
    }

    /// Tab 循环切 active：仅在任务门客之间切；Esc 回玄女。
    pub(crate) fn cycle_active_to_next(&mut self) {
        let mut order = Vec::new();
        for (_, _, members) in self.visible_task_groups() {
            for idx in members {
                let target = ActiveTarget::Worker(self.tasks[idx].worker);
                if !order.contains(&target) {
                    order.push(target);
                }
            }
        }
        if order.is_empty() {
            self.switch_active(ActiveTarget::Xuannv);
            return;
        }
        let cur_pos = order
            .iter()
            .position(|t| *t == self.active)
            .unwrap_or(order.len().saturating_sub(1));
        let next = order[(cur_pos + 1) % order.len()];
        self.switch_active(next);
        self.resync_roster_selection();
    }

    fn current_row_index(&self, rows: &[PaneRow]) -> usize {
        let groups = self.visible_task_groups();
        match self.active {
            ActiveTarget::Xuannv => self.roster_state.selected().unwrap_or(0),
            ActiveTarget::Worker(id) => rows
                .iter()
                .position(|r| match r {
                    PaneRow::Task(i) => self.tasks[*i].worker == id,
                    PaneRow::GroupHeader(_) => false,
                })
                .or_else(|| {
                    let t_idx = self
                        .tasks
                        .iter()
                        .position(|t| t.worker == id && t.worker != self.xuannv_id)?;
                    let key = self.tasks[t_idx].task_id.to_string();
                    rows.iter().position(|r| match r {
                        PaneRow::GroupHeader(gidx) => groups
                            .get(*gidx)
                            .map(|(k, _, _)| k == &key)
                            .unwrap_or(false),
                        PaneRow::Task(_) => false,
                    })
                })
                .unwrap_or(0),
        }
    }

    fn select_row_at(&mut self, rows: &[PaneRow], idx: usize) {
        if let Some(row) = rows.get(idx) {
            let new_active = match row {
                PaneRow::GroupHeader(gidx) => {
                    let groups = self.visible_task_groups();
                    if let Some((key, _, _)) = groups.get(*gidx)
                        && !self.collapsed_task_groups.remove(key)
                    {
                        self.collapsed_task_groups.insert(key.clone());
                    }
                    self.resync_roster_selection();
                    return;
                }
                PaneRow::Task(i) => ActiveTarget::Worker(self.tasks[*i].worker),
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
    /// Worker：对应 task 在跑（thinking 或 state 非终态）。
    pub(crate) fn active_is_busy(&self) -> bool {
        match self.active {
            ActiveTarget::Xuannv => {
                self.xuannv_thinking || matches!(self.xuannv_status, ShelfStatus::Busy)
            }
            ActiveTarget::Worker(id) => {
                self.tasks.iter().filter(|t| t.worker == id).any(|t| {
                    t.thinking || !matches!(t.state, TaskState::Done | TaskState::Cancelled)
                })
            }
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
            "tree" => {
                self.execute_tree_command(arg);
                true
            }
            "kill" => {
                self.run_command_action(crate::command_registry::CommandAction::Kill);
                true
            }
            "status" => {
                self.run_command_action(crate::command_registry::CommandAction::Status);
                true
            }
            "nodes" => {
                self.run_command_action(crate::command_registry::CommandAction::Nodes);
                true
            }
            _ => {
                self.toasts.push(
                    format!("未知命令 /{cmd}，输入 /help 查看可用命令"),
                    crate::toast::ToastVariant::Error,
                    Duration::from_secs(3),
                );
                true
            }
        }
    }

    /// `/nodes` handler：打开拓扑 overlay；其它 overlay 互斥关闭。
    /// 数据由 EventBus 增量维护（公理 3）；overlay 一打开即用最新 state 渲染。
    pub(crate) fn execute_nodes_command(&mut self) {
        self.nodes_overlay_open = true;
        self.help_overlay_open = false;
        self.roster_overlay_open = false;
        self.meta_overlay_open = false;
    }

    /// `/help` handler：打开 help overlay，不往 transcript 写系统行。
    pub(crate) fn execute_help_command(&mut self) {
        self.help_overlay_open = true;
        self.meta_overlay_open = false;
        self.roster_overlay_open = false;
    }

    /// `/tree` handler：
    /// - `on`  开左侧常驻任务树
    /// - `off` 关左侧常驻任务树（回单栏）
    /// - 为空/其它 = toggle
    pub(crate) fn execute_tree_command(&mut self, arg: Option<&str>) {
        let next = match arg {
            Some(a) if a.eq_ignore_ascii_case("on") => true,
            Some(a) if a.eq_ignore_ascii_case("off") => false,
            Some(a) if a.eq_ignore_ascii_case("toggle") => !self.tree_sidebar_enabled,
            Some(a) => {
                self.toasts.push(
                    format!("无效参数 {a}，用 /tree on|off|toggle"),
                    crate::toast::ToastVariant::Error,
                    Duration::from_secs(3),
                );
                return;
            }
            None => !self.tree_sidebar_enabled,
        };
        self.tree_sidebar_enabled = next;
        self.roster_overlay_open = false;
        self.meta_overlay_open = false;
        self.focus = if next { Focus::Roster } else { Focus::Input };
        self.toasts.push(
            if next {
                "已开启左侧任务树"
            } else {
                "已关闭左侧任务树"
            },
            crate::toast::ToastVariant::Success,
            Duration::from_secs(2),
        );
    }

    /// 统一 action 路由——popup 吐 `Execute(action)` 时调这个。
    pub(crate) fn run_command_action(&mut self, action: crate::command_registry::CommandAction) {
        use crate::command_registry::CommandAction;
        match action {
            CommandAction::Help => self.execute_help_command(),
            CommandAction::Theme(name) => self.execute_theme_command(name.as_deref()),
            CommandAction::Tree => {
                self.execute_tree_command(None);
            }
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
            CommandAction::Kill => match self.active {
                ActiveTarget::Worker(id) => {
                    self.pending_submit = Some(Submit::Kill(id));
                    self.toasts.push(
                        "正在下线当前门客…",
                        crate::toast::ToastVariant::Info,
                        Duration::from_secs(2),
                    );
                }
                ActiveTarget::Xuannv => {
                    self.toasts.push(
                        "玄女不可被 /kill",
                        crate::toast::ToastVariant::Error,
                        Duration::from_secs(3),
                    );
                }
            },
            CommandAction::Status => {
                if self.tree_sidebar_enabled {
                    self.focus = Focus::Roster;
                } else {
                    self.roster_overlay_open = true;
                    self.meta_overlay_open = false;
                    self.help_overlay_open = false;
                    self.focus = Focus::Roster;
                }
            }
            CommandAction::Nodes => self.execute_nodes_command(),
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
            self.toasts.push(
                "中断请求已发",
                crate::toast::ToastVariant::Success,
                Duration::from_secs(3),
            );
            self.esc_last_at = None;
            self.esc_count = 0;
        } else {
            self.esc_last_at = Some(now);
            self.esc_count = 1;
            self.toasts.push(
                "再按一次 Esc 确认中断",
                crate::toast::ToastVariant::Info,
                Duration::from_secs(2),
            );
        }
    }

    fn roster_up(&mut self) {
        let rows = self.pane_rows();
        if rows.is_empty() {
            return;
        }
        let cur = self.roster_state.selected().unwrap_or(0);
        self.roster_state.select(Some(cur.saturating_sub(1)));
    }

    fn roster_down(&mut self) {
        let rows = self.pane_rows();
        if rows.is_empty() {
            return;
        }
        let cur = self.roster_state.selected().unwrap_or(0);
        self.roster_state
            .select(Some((cur + 1).min(rows.len() - 1)));
    }

    fn roster_enter(&mut self) {
        let rows = self.pane_rows();
        let Some(idx) = self.roster_state.selected() else {
            return;
        };
        let is_group = matches!(rows.get(idx), Some(PaneRow::GroupHeader(_)));
        self.select_row_at(&rows, idx);
        if !is_group {
            self.focus = Focus::Input;
        }
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
            let in_window = self
                .ctrl_c_last_at
                .map(|t| now.saturating_duration_since(t) <= CTRL_C_DOUBLE_WINDOW)
                .unwrap_or(false);
            if in_window && self.ctrl_c_count >= 1 {
                self.should_quit = true;
                self.ctrl_c_last_at = None;
                self.ctrl_c_count = 0;
            } else {
                self.ctrl_c_last_at = Some(now);
                self.ctrl_c_count = 1;
                self.toasts.push(
                    "再按一次 Ctrl-C 退出",
                    crate::toast::ToastVariant::Info,
                    Duration::from_secs(2),
                );
            }
            return None;
        }
        self.ctrl_c_last_at = None;
        self.ctrl_c_count = 0;
        if has_os_shortcut_modifier(mods) {
            // 让 Cmd/Ctrl 系统级快捷键（尤其 Cmd+C）不污染输入框。
            return None;
        }
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
                crate::autocomplete::PopupEvent::None => {
                    self.input = new_textarea();
                    self.input.insert_str(self.popup.display_input());
                    self.focus = Focus::Input;
                }
                crate::autocomplete::PopupEvent::Close => {
                    self.input = new_textarea();
                    self.focus = Focus::Input;
                }
                crate::autocomplete::PopupEvent::CompleteInput(s) => {
                    self.input = new_textarea();
                    self.input.insert_str(&s);
                    self.focus = Focus::Input;
                }
                crate::autocomplete::PopupEvent::Execute(action) => {
                    self.input = new_textarea();
                    self.run_command_action(action);
                    return self.pending_submit.take();
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
            self.input = new_textarea();
            self.input.insert_str("/");
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
                if self.roster_overlay_open
                    || self.meta_overlay_open
                    || self.help_overlay_open
                    || self.nodes_overlay_open
                {
                    self.roster_overlay_open = false;
                    self.meta_overlay_open = false;
                    self.help_overlay_open = false;
                    self.nodes_overlay_open = false;
                    return None;
                }
                self.handle_esc_at(now);
                return None;
            }
            KeyCode::F(2) => {
                self.events_visible = !self.events_visible;
                return None;
            }
            KeyCode::F(4) => {
                if self.tree_sidebar_enabled {
                    // 常驻树模式：F4 仅切焦点，不再开浮层。
                    self.focus = if self.focus == Focus::Roster {
                        Focus::Input
                    } else {
                        Focus::Roster
                    };
                } else {
                    // 旧模式：roster overlay 切换。
                    self.roster_overlay_open = !self.roster_overlay_open;
                    if self.roster_overlay_open {
                        self.meta_overlay_open = false;
                        self.help_overlay_open = false;
                        self.focus = Focus::Roster;
                    } else {
                        self.focus = Focus::Input;
                    }
                }
                return None;
            }
            KeyCode::F(5) => {
                // meta overlay 是只读展示——焦点不跟着转，保持在 input 方便继续打字。
                self.meta_overlay_open = !self.meta_overlay_open;
                if self.meta_overlay_open {
                    self.roster_overlay_open = false;
                    self.help_overlay_open = false;
                    self.nodes_overlay_open = false;
                }
                return None;
            }
            KeyCode::F(6) => {
                // /nodes 拓扑 overlay。同 meta，不抢焦点。
                self.nodes_overlay_open = !self.nodes_overlay_open;
                if self.nodes_overlay_open {
                    self.roster_overlay_open = false;
                    self.meta_overlay_open = false;
                    self.help_overlay_open = false;
                }
                return None;
            }
            KeyCode::Up if self.nodes_overlay_open => {
                self.nodes_selected = self.nodes_selected.saturating_sub(1);
                return None;
            }
            KeyCode::Down if self.nodes_overlay_open => {
                let max = self.nodes.len().saturating_sub(1);
                if self.nodes_selected < max {
                    self.nodes_selected += 1;
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
            KeyCode::Char('v') if mods == KeyModifiers::CONTROL => {
                self.paste_from_system_clipboard();
                None
            }
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
                let text_raw = self.take_input();
                let visible_trimmed = text_raw.trim();
                if visible_trimmed.is_empty() {
                    return None;
                }
                // slash 命令拦截——/theme 等不走 Xuannv/Worker，直接在本地做。
                // WHY 在 Enter 路径里拦：popup（#17）还没接 repl；先走最小接线，
                // popup 接入后再把 `CommandAction::Theme(name)` 也路由到同一个 handler。
                if self.try_handle_slash_submit(visible_trimmed) {
                    self.history.push(visible_trimmed);
                    return self.pending_submit.take();
                }
                // 记一条历史——提交后 ↑ 能回翻本句。push 有连续去重。
                self.history.push(visible_trimmed);
                match self.active {
                    ActiveTarget::Xuannv => Some(Submit::Xuannv(visible_trimmed.to_string())),
                    ActiveTarget::Worker(id) => {
                        Some(Submit::Worker(id, visible_trimmed.to_string()))
                    }
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
        // 避免状态事件顺序造成的锚点漏记/残留：每帧按当前 busy 态重整一次。
        self.refresh_xuannv_busy_anchor();

        // 活状态行 spinner 节流：20Hz 主循环下 3 帧推进一次，约 6~7fps，
        // 比 4fps 更有活力，又不会像每帧跳动那样制造焦虑感。
        if self.active_is_busy() {
            self.spinner_tick_gate = self.spinner_tick_gate.wrapping_add(1);
            if self.spinner_tick_gate.is_multiple_of(3) {
                self.status_spinner.tick();
            }
        } else {
            self.spinner_tick_gate = 0;
        }
        if self
            .tasks
            .iter()
            .any(|t| task_state_to_shelf(t.state, t.thinking) == ShelfStatus::Busy)
        {
            self.teammate_tree_tick = self.teammate_tree_tick.wrapping_add(1);
        }

        let root_area = if self.tree_sidebar_enabled {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(34), Constraint::Min(20)])
                .split(f.area());
            self.draw_roster(f, cols[0]);
            self.click.register(cols[0], ClickAction::FocusRoster);
            // 左树和对话区之间固定竖分隔，避免视觉融合。
            f.render_widget(
                Block::default()
                    .borders(Borders::LEFT)
                    .border_style(Style::default().fg(theme().dim_border())),
                cols[1],
            );
            Rect {
                x: cols[1].x.saturating_add(1),
                y: cols[1].y,
                width: cols[1].width.saturating_sub(1),
                height: cols[1].height,
            }
        } else {
            f.area()
        };

        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(if self.events_visible { 10 } else { 0 }),
                Constraint::Length(4),
                Constraint::Length(1),
            ])
            .split(root_area);
        self.draw_dialogue(f, root[0]);
        self.click.register(root[0], ClickAction::FocusDialogue);
        if self.events_visible {
            self.draw_events(f, root[1]);
        }
        self.draw_input(f, root[2]);
        self.click.register(root[2], ClickAction::FocusInput);
        self.draw_status(f, root[3]);

        // overlay 浮层——优先级：roster > help > meta > nodes（互斥，同时只一个开着）。
        // 渲染顺序：overlay 在 toast 之前——toast 始终最顶。
        if !self.tree_sidebar_enabled && self.roster_overlay_open {
            self.draw_roster_overlay(f, f.area());
        } else if self.help_overlay_open {
            self.draw_help_overlay(f, f.area());
        } else if self.meta_overlay_open {
            self.draw_meta_overlay(f, f.area());
        } else if self.nodes_overlay_open {
            self.draw_nodes_overlay(f, f.area());
        }

        // slash popup——在 input 正上方贴条，40%-60% 屏宽居中。
        // 位置在 overlay 之后、toast 之前：overlay 盖底层，toast 最顶，popup 居中。
        if self.popup.is_open() {
            self.draw_popup(f, root[2]);
        }

        // Toast 最后画——要浮在所有 pane 和 overlay 之上。放在输入框上沿附近，
        // 用户视线不用跳到右上角。
        self.draw_toasts(f, root[2]);
    }

    /// 在 input_area 正上方渲染 SlashPopup，**anchor 到 input 左下对齐**向上生长。
    ///
    /// WHY 贴 input 而非居中浮在屏幕中央：2026-04-21 用户反馈——用户正在输入框
    /// 键入 `/`，popup 在屏幕中心会让眼球跨越视觉重心两次（输入 → 中央 →
    /// 回输入）。参考 VS Code autocomplete / fish shell 的 inline suggestion，
    /// popup 应该**紧贴输入框**，宽度匹配输入框，眼球只在一条水平线上移动。
    ///
    /// 布局：
    /// - `x = input_area.x`（和输入框左对齐）
    /// - `width = input_area.width`（和输入框同宽）
    /// - `height = min(候选数 + 2 边框, 12)`（不压到对话区超过半屏）
    /// - `y = input_area.y - height`（贴 input 顶边向上浮）
    ///
    /// filter 文本不塞进 popup title——用户看到的 filter 实际是**输入框内**
    /// 的 `/h█` 这种（textarea 已显示），popup 只展示候选。
    fn draw_popup(&self, f: &mut ratatui::Frame<'_>, input_area: Rect) {
        let t = theme();
        let lines = self.popup.render_lines(&t);
        // 候选行数 + 边框 2；最多 10 行避免压过半屏。
        let desired_rows = (lines.len() as u16).max(1).saturating_add(2);
        let height = desired_rows.min(10).min(input_area.y); // 不能浮到负 y
        let width = input_area.width;
        let x = input_area.x;
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

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(t.focus_border()));

        let body: Vec<Line<'_>> = if lines.is_empty() {
            vec![Line::from("（无匹配命令）")]
        } else {
            lines
        };
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

    fn draw_help_overlay(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rect = Self::overlay_rect(area, 62, 72);
        f.render_widget(ratatui::widgets::Clear, rect);
        self.draw_help(f, rect);
    }

    /// /nodes 拓扑 overlay——比 meta 宽（要容 6 列），比 help 短（行数随节点）。
    fn draw_nodes_overlay(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rect = Self::overlay_rect(area, 72, 60);
        f.render_widget(ratatui::widgets::Clear, rect);
        self.draw_nodes(f, rect);
    }

    /// 拓扑表格本体。空态有引导提示；有节点时 6 列表格 + 选中行高亮 + 状态栏。
    /// **不**轮询——本函数仅基于 self.nodes 渲染，state 由 ingest() 外部刷。
    fn draw_nodes(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let t = theme();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 远端 worker 拓扑 ")
            .border_style(Style::default().fg(t.focus_border()));

        if self.nodes.is_empty() {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "无远端 worker",
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(ratatui::layout::Alignment::Center),
                Line::from(""),
                Line::from(Span::styled(
                    "fuxi up --node <name> --controller https://...",
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(ratatui::layout::Alignment::Center),
                Line::from(Span::styled(
                    "可注册一个",
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(ratatui::layout::Alignment::Center),
            ];
            f.render_widget(Paragraph::new(lines).block(block), area);
            return;
        }

        // 表头 + 数据行 + 状态栏。固定列宽，超长截断 `…`。
        // 列宽：NODE 12 / STATUS 8 / TAGS 22 / IN/CAP 8 / LAST 11 / REG 12（合计 73 + 5 间隔 = 78，80 列基线）
        let header = Line::from(vec![
            Span::styled(
                format!("{:<12} ", "NODE"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<8} ", "STATUS"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<22} ", "TAGS"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<8} ", "IN/CAP"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<11} ", "LAST_SEEN"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<12}", "REGISTERED"),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let now = Instant::now();
        let mut alive = 0u32;
        let mut stale = 0u32;
        let sel = self.nodes_selected.min(self.nodes.len().saturating_sub(1));

        let mut lines: Vec<Line<'static>> = vec![header, Line::from("")];
        for (idx, n) in self.nodes.iter().enumerate() {
            match n.status {
                NodeStatus::Alive => alive += 1,
                NodeStatus::Stale => stale += 1,
                NodeStatus::Unknown => {}
            }
            let (mark, mark_color, status_text) = match n.status {
                NodeStatus::Alive => ("●", Color::Green, "alive"),
                NodeStatus::Stale => ("○", Color::Red, "stale"),
                NodeStatus::Unknown => ("·", Color::DarkGray, "?"),
            };
            let tags_str = {
                let joined = n.tags.join(",");
                if joined.chars().count() > 20 {
                    format!("{}+{}", short_str(&joined, 17), n.tags.len())
                } else {
                    joined
                }
            };
            let inflight_color = if n.max_concurrency > 0 && n.inflight >= n.max_concurrency {
                Color::Yellow
            } else {
                Color::Reset
            };
            let last_seen = humanize_elapsed_live(now.duration_since(n.last_event_at));
            let registered = humanize_elapsed_live(now.duration_since(n.last_event_at));
            // ↑ register_at 暂用 last_event_at 兜底——α apply_snapshot 灌入时会单独写真值
            let prefix = if idx == sel { "▶" } else { " " };

            let row = Line::from(vec![
                Span::raw(format!("{prefix}{:<11} ", short_str(&n.node_id, 11))),
                Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
                Span::styled(
                    format!("{:<6}", status_text),
                    Style::default().fg(mark_color),
                ),
                Span::raw(format!("{:<22} ", short_str(&tags_str, 22))),
                Span::styled(
                    format!("{:>3}/{:<4} ", n.inflight, n.max_concurrency),
                    Style::default().fg(inflight_color),
                ),
                Span::raw(format!("{:<11} ", short_str(&last_seen, 11))),
                Span::raw(format!("{:<12}", short_str(&registered, 12))),
            ]);
            if idx == sel {
                lines.push(
                    row.style(Style::default().add_modifier(ratatui::style::Modifier::REVERSED)),
                );
            } else {
                lines.push(row);
            }
        }

        // 状态栏（最后一行）
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("▍ ", Style::default().fg(t.focus_border())),
            Span::styled(format!("{alive} alive"), Style::default().fg(Color::Green)),
            Span::raw(" · "),
            Span::styled(format!("{stale} stale"), Style::default().fg(Color::Red)),
            Span::raw(format!(" · 总 {} ", self.nodes.len())),
            Span::raw(" · "),
            Span::styled(
                "F6 关 / ↑↓ 选 / Esc 关",
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        f.render_widget(Paragraph::new(lines).block(block), area);
    }

    /// Toast 层渲染。锚在输入框上方几行，避免用户注意力跳到屏幕右上。
    fn draw_toasts(&self, f: &mut ratatui::Frame<'_>, input_area: Rect) {
        if input_area.width < 12 {
            return;
        }
        let visible = self.toasts.len().min(crate::toast::TOAST_MAX) as u16;
        if visible == 0 {
            return;
        }
        let h = visible.min(input_area.y).max(1);
        let toast_area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(h),
            width: input_area.width,
            height: h,
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
                    self.selection_dragged = false;
                } else {
                    self.selection_anchor = None;
                    self.selection_cursor = None;
                    self.selection_dragged = false;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selection_anchor.is_some() {
                    // Drag 可能越出 dialogue area；终点位置自然 clamp 在 render 时做。
                    self.selection_cursor = Some((ev.column, ev.row));
                    self.selection_dragged = true;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let (Some(a), Some(b)) =
                    (self.selection_anchor.take(), self.selection_cursor.take())
                    && self.selection_dragged
                    && a != b
                {
                    self.finish_selection_copy(a, b);
                }
                self.selection_dragged = false;
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
    /// 不精确到 char-level：v1 保持 cell 级选区即可。
    pub(crate) fn extract_selected_text(
        &self,
        area: Rect,
        anchor: (u16, u16),
        cursor: (u16, u16),
    ) -> String {
        let inner_y = area.y;
        let inner_h = area.height;
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
        let row_a = to_row(anchor.1) as usize;
        let row_b = to_row(cursor.1) as usize;
        let col_a = anchor.0.saturating_sub(area.x) as usize;
        let col_b = cursor.0.saturating_sub(area.x) as usize;

        let scroll = self.dialogue_scroll as usize;
        let abs_a = scroll + row_a;
        let abs_b = scroll + row_b;

        let ((start_row, start_col), (end_row, end_col)) = if (abs_a, col_a) <= (abs_b, col_b) {
            ((abs_a, col_a), (abs_b, col_b))
        } else {
            ((abs_b, col_b), (abs_a, col_a))
        };

        let rows = if self.last_dialogue_wrapped_rows.is_empty() {
            let empty = VecDeque::new();
            let bucket = self.dialogues.get(&self.active).unwrap_or(&empty);
            let lines: Vec<Line<'_>> = render_dialogue_collapsed(bucket.iter());
            collect_wrapped_plain_rows(&lines, area.width)
        } else {
            self.last_dialogue_wrapped_rows.clone()
        };

        let mut out = Vec::new();
        for row in start_row..=end_row {
            let Some(line) = rows.get(row) else {
                continue;
            };
            let text = if start_row == end_row {
                slice_by_display_cols(line, start_col, end_col.saturating_add(1))
            } else if row == start_row {
                slice_by_display_cols(line, start_col, usize::MAX)
            } else if row == end_row {
                slice_by_display_cols(line, 0, end_col.saturating_add(1))
            } else {
                line.clone()
            };
            if !text.is_empty() {
                out.push(text);
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
        let groups = self.visible_task_groups();
        let rows = self.pane_rows();
        let selected = self.roster_state.selected();
        let active_row_idx = self.current_row_index(&rows);
        let items: Vec<ListItem> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| match row {
                PaneRow::GroupHeader(gidx) => {
                    let (key, title, members) = &groups[*gidx];
                    let collapsed = self.collapsed_task_groups.contains(key);
                    let agg_state = members
                        .iter()
                        .map(|idx| {
                            let t = &self.tasks[*idx];
                            task_state_to_shelf(t.state, t.thinking)
                        })
                        .max_by_key(|s| match s {
                            ShelfStatus::Busy => 3,
                            ShelfStatus::Idle => 2,
                            ShelfStatus::Dead => 1,
                        })
                        .unwrap_or(ShelfStatus::Idle);
                    let rhs = if members.iter().all(|idx| {
                        matches!(
                            self.tasks[*idx].state,
                            TaskState::Done | TaskState::Cancelled
                        )
                    }) {
                        "已完".to_string()
                    } else {
                        members
                            .iter()
                            .map(|idx| self.tasks[*idx].dispatched_at.elapsed())
                            .max()
                            .map(humanize_elapsed)
                            .unwrap_or_else(|| "0s".to_string())
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            if collapsed { "▸ " } else { "▾ " },
                            Style::default().fg(Color::DarkGray),
                        ),
                        status_marker_span(agg_state),
                        Span::raw(" "),
                        Span::styled(
                            truncate_by_width(title, 14),
                            Style::default().fg(theme().subtext0),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            format!("{}门客 · {}", members.len(), rhs),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                }
                PaneRow::Task(idx) => {
                    let t = &self.tasks[*idx];
                    let marker = if matches!(t.state, TaskState::Done | TaskState::Cancelled) {
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
                    let role_color = if matches!(t.state, TaskState::Done | TaskState::Cancelled) {
                        theme().muted()
                    } else {
                        theme().subtext0
                    };
                    let (group_size, pos_in_group, role_name) = groups
                        .iter()
                        .find_map(|(_, _, members)| {
                            members
                                .iter()
                                .position(|member_idx| *member_idx == *idx)
                                .map(|pos| {
                                    let role_seen = members
                                        .iter()
                                        .take(pos + 1)
                                        .filter(|member_idx| {
                                            self.tasks[**member_idx].worker_role == t.worker_role
                                        })
                                        .count();
                                    let role_total = members
                                        .iter()
                                        .filter(|member_idx| {
                                            self.tasks[**member_idx].worker_role == t.worker_role
                                        })
                                        .count();
                                    let base = self.role_display(&t.worker_role);
                                    let shown = if role_total >= 2 && role_seen >= 2 {
                                        format!("{base}#{role_seen}")
                                    } else {
                                        base
                                    };
                                    (members.len(), pos, shown)
                                })
                        })
                        .unwrap_or_else(|| (1, 0, self.role_display(&t.worker_role)));
                    let role_with_desc = if !t.description.trim().is_empty() {
                        format!(
                            "{} · {}",
                            role_name,
                            truncate_by_width(t.description.trim(), 8)
                        )
                    } else {
                        role_name
                    };
                    let recent = t
                        .recent_tools
                        .back()
                        .map(|s| truncate_by_width(s, 14))
                        .unwrap_or_else(|| "待命".to_string());
                    let branch = if group_size >= 2 {
                        if pos_in_group + 1 == group_size {
                            "└ "
                        } else {
                            "├ "
                        }
                    } else {
                        "  "
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(active_mark, Style::default().fg(theme().focus_border())),
                        Span::styled(branch, Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            truncate_by_width(&role_with_desc, 14),
                            Style::default().fg(role_color),
                        ),
                        Span::raw("  "),
                        marker,
                        Span::raw(" · "),
                        Span::styled(recent, Style::default().fg(Color::DarkGray)),
                    ]))
                }
            })
            .collect();
        let block = Block::default()
            .borders(Borders::TOP)
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
        f.render_widget(block, area);
        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let list_items = if items.is_empty() {
            vec![ListItem::new(Line::from(vec![Span::styled(
                "（暂无任务门客）",
                Style::default().fg(Color::DarkGray),
            )]))]
        } else {
            items
        };
        let list = List::new(list_items)
            .highlight_style(Style::default().bg(theme().surface0).fg(theme().text));
        let _ = selected;
        f.render_stateful_widget(list, inner, &mut self.roster_state);
    }

    #[cfg(test)]
    fn busy_tasks(&self) -> Vec<&TaskNode> {
        self.tasks
            .iter()
            .filter(|t| task_state_to_shelf(t.state, t.thinking) == ShelfStatus::Busy)
            .collect()
    }

    #[cfg(test)]
    fn teammate_task_tree_lines(&self, tasks: &[&TaskNode]) -> Vec<String> {
        if tasks.is_empty() {
            return Vec::new();
        }

        // 按 task_id 归组：同标题不同任务必须分开。
        let mut groups: Vec<(String, String, Vec<&TaskNode>)> = Vec::new();
        for t in tasks {
            let key = t.task_id.to_string();
            let title = if t.title.is_empty() {
                "任务".to_string()
            } else {
                t.title.clone()
            };
            if let Some((_, _, members)) = groups.iter_mut().find(|(k, _, _)| *k == key) {
                members.push(*t);
            } else {
                groups.push((key, title, vec![*t]));
            }
        }

        let mut out = Vec::new();
        for (gidx, (_, title, members)) in groups.iter().enumerate() {
            let elapsed = members
                .iter()
                .map(|t| t.dispatched_at.elapsed())
                .max()
                .map(humanize_elapsed)
                .unwrap_or_else(|| "0s".to_string());
            let root_glyph = status_marker(ShelfStatus::Busy);
            out.push(format!(
                "▾ {root_glyph} {}  {}",
                truncate_by_width(title, 20),
                elapsed
            ));
            let mut role_total: HashMap<String, usize> = HashMap::new();
            for t in members {
                *role_total.entry(t.worker_role.clone()).or_insert(0) += 1;
            }
            let mut role_seen: HashMap<String, usize> = HashMap::new();
            for (midx, t) in members.iter().enumerate() {
                let nth = role_seen.entry(t.worker_role.clone()).or_insert(0);
                *nth += 1;
                let role_name =
                    if role_total.get(&t.worker_role).copied().unwrap_or(0) >= 2 && *nth >= 2 {
                        format!("{}#{}", t.worker_role, *nth)
                    } else {
                        t.worker_role.clone()
                    };
                let worker_glyph = status_marker(task_state_to_shelf(t.state, t.thinking));
                let verb = crate::spinner::xuannv_verb_by_tick(
                    self.teammate_tree_tick + (gidx + midx) as u64,
                );
                let detail = if !t.description.trim().is_empty() {
                    truncate_by_width(t.description.trim(), 12)
                } else {
                    t.recent_tools
                        .back()
                        .map(|s| truncate_by_width(s, 20))
                        .unwrap_or_else(|| "待命".to_string())
                };
                let branch = if midx + 1 == members.len() {
                    "└"
                } else {
                    "├"
                };
                out.push(format!(
                    "   {branch} {worker_glyph} {} · {} · {}",
                    truncate_by_width(&role_name, 10),
                    verb,
                    detail
                ));
            }
        }
        out
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
            .copied()
            .collect();
        let collapsed = collapse_consecutive_tools(&filtered);
        let available = inner.height as usize;
        let start = collapsed.len().saturating_sub(available);
        let lines: Vec<Line> = collapsed[start..]
            .iter()
            .map(|row| {
                // 时间 (8) + icon (2) + who (6) + 3 空格 = 19 预留给头部
                let reserved = 20u16;
                let narrative_width = inner.width.saturating_sub(reserved).max(10) as usize;
                Line::from(vec![
                    Span::styled(row.time.clone(), Style::default().fg(t.muted())),
                    Span::raw(" "),
                    Span::styled(
                        row.icon,
                        Style::default().fg(row.color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(short_str(&row.who, 6), Style::default().fg(t.info())),
                    Span::raw(" "),
                    Span::styled(
                        truncate_by_width(&row.narrative, narrative_width),
                        Style::default().fg(row.color),
                    ),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn draw_dialogue(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let inner = area;

        let empty = VecDeque::new();
        let bucket = self.dialogues.get(&self.active).unwrap_or(&empty);
        let lines: Vec<Line> = render_dialogue_collapsed(bucket.iter());
        self.last_dialogue_wrapped_rows = collect_wrapped_plain_rows(&lines, inner.width);

        let inner_h = inner.height;
        // 滚动总行数按同一 wrapped 规则统一计算，避免逻辑行和屏幕行模型混用。
        let total = lines
            .iter()
            .fold(0u32, |acc, line| {
                acc.saturating_add(count_wrapped_rows(line, inner.width) as u32)
            })
            .max(1)
            .min(u16::MAX as u32) as u16;
        self.last_dialogue_total = total;
        self.last_dialogue_view = inner_h;
        if self.dialogue_auto_scroll {
            self.dialogue_scroll = total.saturating_sub(inner_h);
        } else {
            let max = total.saturating_sub(inner_h);
            if self.dialogue_scroll > max {
                self.dialogue_scroll = max;
            }
            // 当用户当前已在底部（或贴底 1 行内）时，恢复 auto-follow。
            if self.dialogue_scroll >= max.saturating_sub(1) {
                self.dialogue_auto_scroll = true;
            }
        }

        let para = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.dialogue_scroll, 0));
        f.render_widget(para, area);

        // 记 area 以便鼠标 drag 把 cell 坐标 → 对话行映射。
        self.last_dialogue_area = Some(inner);

        // 选中范围 overlay：对选中 cells 加 REVERSED modifier，用户视觉上看到
        // 被"反色"的选区。不精确（整行 cell）但对 v1 剪贴板够用。
        if self.selection_dragged
            && let (Some(a), Some(b)) = (self.selection_anchor, self.selection_cursor)
            && let Some(inner) = self.last_dialogue_area
        {
            apply_selection_reverse(f.buffer_mut(), inner, a, b);
        }
    }

    fn draw_input(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let (background_busy, _) = self.background_busy_count();
        let status_line = if self.active_is_busy() {
            let glyph = self.status_spinner.glyph();
            let verb = self
                .active_elapsed()
                .map(|d| crate::spinner::xuannv_verb_by_tick(d.as_secs() / 4))
                .unwrap_or("思考中");
            let elapsed = self
                .active_elapsed()
                .map(humanize_elapsed_live)
                .unwrap_or_else(|| "0s".to_string());
            if background_busy > 0 {
                format!(" {glyph} {verb} · {elapsed} · 后台 {background_busy} ")
            } else {
                format!(" {glyph} {verb} · {elapsed} ")
            }
        } else if background_busy > 0 {
            format!(" ● 已就绪 · 后台 {background_busy} ")
        } else {
            String::new()
        };
        let show_status = !status_line.is_empty();
        let chunks = if show_status {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(1)])
                .split(area)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(0), Constraint::Min(1)])
                .split(area)
        };
        if show_status {
            f.render_widget(
                Paragraph::new(status_line).style(Style::default().fg(theme().subtext0)),
                chunks[0],
            );
        }

        let mut ta_widget = self.input.clone();
        ta_widget.set_block(Block::default().borders(Borders::TOP).border_style(
            Style::default().fg(if self.focus == Focus::Input {
                theme().focus_border()
            } else {
                theme().dim_border()
            }),
        ));
        f.render_widget(&ta_widget, chunks[1]);
    }

    fn draw_status(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let t = theme();
        let bg = Style::default().fg(t.subtext1).bg(t.mantle);
        let target = match self.active {
            ActiveTarget::Xuannv => "玄女".to_string(),
            ActiveTarget::Worker(id) => self.agent_display_name(id),
        };
        let para = Paragraph::new(format!(" 你 → {target}")).style(bg);
        f.render_widget(para, area);
    }

    fn background_busy_count(&self) -> (usize, bool) {
        let total_busy = self
            .tasks
            .iter()
            .filter(|task| task_state_to_shelf(task.state, task.thinking) == ShelfStatus::Busy)
            .count();
        let active_busy = self.active_is_busy();
        let background_busy = match self.active {
            ActiveTarget::Worker(id) if active_busy => self
                .tasks
                .iter()
                .filter(|task| task.worker != id)
                .filter(|task| task_state_to_shelf(task.state, task.thinking) == ShelfStatus::Busy)
                .count(),
            _ => total_busy,
        };
        (background_busy, active_busy)
    }

    /// 当前 active 对象忙了多久（Xuannv 看 busy_since；Worker 看 dispatched_at）。
    fn active_elapsed(&self) -> Option<Duration> {
        match self.active {
            ActiveTarget::Xuannv => self.xuannv_busy_since.map(|t| t.elapsed()),
            ActiveTarget::Worker(id) => self
                .tasks
                .iter()
                .filter(|t| t.worker == id)
                .filter(|t| !matches!(t.state, TaskState::Done | TaskState::Cancelled))
                .max_by_key(|t| t.dispatched_at)
                .map(|t| t.dispatched_at.elapsed()),
        }
    }

    fn draw_meta(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let (lines, title) = match self.active {
            ActiveTarget::Xuannv => {
                let task_line =
                    if let Some(t) = self.tasks.iter().find(|t| t.worker == self.xuannv_id) {
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
                            if matches!(t.state, TaskState::Done | TaskState::Cancelled) {
                                "-".to_string()
                            } else {
                                humanize_elapsed(t.dispatched_at.elapsed())
                            }
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
                } else if let Some(role) = self.roles_by_agent.get(&id) {
                    (
                        vec![
                            Line::from(format!("worker   {}", short_id_of(id))),
                            Line::from(format!("role     {}", truncate_by_width(role, 16))),
                            Line::from(Span::styled(
                                "（未绑定任务）",
                                Style::default().fg(Color::DarkGray),
                            )),
                        ],
                        " 门客 · 元信息 ",
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

    fn draw_help(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let t = theme();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 命令帮助 ")
            .border_style(Style::default().fg(t.focus_border()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines: Vec<Line<'_>> = self
            .cmd_registry
            .render_help_markdown()
            .lines()
            .map(|s| Line::from(s.to_string()))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from("Esc 关闭帮助"));
        f.render_widget(
            Paragraph::new(lines)
                .style(Style::default().fg(t.text))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
}

/// 对话区首行锚点格式：「`▍ HH:MM `」= 竖条 1 + 空格 1 + 5 字 + 空格 1 = 8 宽。
/// 续行用 8 个空格对齐，视觉上续行"挂"在首行内容下方。
const ANCHOR_WIDTH: usize = 2;

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
/// WHY ANCHOR_WIDTH = 2：CC 风格简前缀（如 `› ` / `● `）只占 2 列，强调角色而非时间。
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
    let entries: Vec<&DialogueEntry> = iter.into_iter().collect();
    let mut i = 0usize;
    let mut prev_kind: Option<DialogueKind> = None;
    while i < entries.len() {
        let entry = entries[i];
        let kind = dialogue_kind(&entry.line);
        if let Some(prev) = prev_kind
            && prev != kind
        {
            out.push(Line::from(""));
        }
        prev_kind = Some(kind);

        match &entry.line {
            DialogueLine::User(t) => {
                push_anchored(
                    &mut out,
                    t,
                    "› ",
                    th.subtext0,
                    false,
                    Style::default().fg(th.text).bg(th.surface0),
                );
            }
            DialogueLine::Agent { name: _, text } => {
                let (kind, first_stripped) = classify_agent_text(text);
                // 合并相邻**同 kind** 的 agent 行——分布式 progress 会把一次
                // turn 内的 tool 输出拆成多条 chunk，每条一个 AgentResponded
                // 事件；渲染时合成一个带 rail 的块视觉干净。
                // Assistant 走 markdown，段落分隔是 `\n\n`；其他 kind 纯文本用 `\n`。
                let sep = if matches!(kind, AgentKind::Assistant) {
                    "\n\n"
                } else {
                    "\n"
                };
                let mut merged = first_stripped.to_string();
                let mut j = i + 1;
                while j < entries.len() {
                    if let DialogueLine::Agent { name: _, text: t2 } = &entries[j].line {
                        let (k2, s2) = classify_agent_text(t2);
                        if k2 == kind {
                            merged.push_str(sep);
                            merged.push_str(s2);
                            j += 1;
                            continue;
                        }
                    }
                    break;
                }
                // Tool 输出照抄 opencode：硬截 10 行——长命令回显裸露太多
                // 会把 assistant 文本挤出屏外，读起来更散。
                let merged = if matches!(kind, AgentKind::Tool) {
                    truncate_lines(&merged, 10)
                } else {
                    merged
                };
                let sty = style_for_agent_kind(kind, &th);
                match kind {
                    AgentKind::Assistant => {
                        // Assistant 文本走 markdown 渲染——bold/italic/inline code/代码块等
                        // 转成 Ratatui Spans；tool/thinking/error 仍走纯文本（tool 是
                        // shell output，markdown 语法会误伤；thinking/error 短多单行）。
                        let md_lines = crate::markdown::md_to_lines(&merged, sty.body, &th);
                        push_lines_with_rail(
                            &mut out,
                            md_lines,
                            sty.anchor,
                            sty.anchor_color,
                            sty.bold_first,
                            Some(('│', sty.rail_color)),
                        );
                    }
                    _ => {
                        push_anchored_with_rail(
                            &mut out,
                            &merged,
                            sty.anchor,
                            sty.anchor_color,
                            sty.bold_first,
                            sty.body,
                            Some(('│', sty.rail_color)),
                        );
                    }
                }
                i = j;
                continue;
            }
            DialogueLine::Tool { text, ok } => {
                let mut tool_texts = vec![text.clone()];
                let mut j = i + 1;
                while j < entries.len() {
                    match &entries[j].line {
                        DialogueLine::Tool { text: t2, ok: ok2 } if *ok2 == *ok => {
                            tool_texts.push(t2.clone());
                            j += 1;
                        }
                        _ => break,
                    }
                }
                push_anchored(
                    &mut out,
                    &tool_texts[0],
                    "● ",
                    th.tool_call(),
                    false,
                    Style::default().fg(if *ok { th.tool_call() } else { th.error() }),
                );
                if tool_texts.len() >= 2 {
                    let mut tail = tool_texts[1].clone();
                    if tool_texts.len() >= 3 {
                        tail.push_str(&format!(" (+{} more)", tool_texts.len() - 2));
                    }
                    out.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled("└ ", Style::default().fg(th.muted())),
                        Span::styled(
                            tail,
                            Style::default().fg(if *ok { th.subtext1 } else { th.error() }),
                        ),
                    ]));
                }
                i = j;
                continue;
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
        i += 1;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogueKind {
    User,
    Agent,
    Tool,
    System,
}

fn dialogue_kind(line: &DialogueLine) -> DialogueKind {
    match line {
        DialogueLine::User(_) => DialogueKind::User,
        DialogueLine::Agent { .. } => DialogueKind::Agent,
        DialogueLine::Tool { .. } => DialogueKind::Tool,
        DialogueLine::System(_) => DialogueKind::System,
    }
}

fn is_agent_error_text(text: &str) -> bool {
    let s = text.to_ascii_lowercase();
    s.starts_with("api error")
        || s.starts_with("error:")
        || s.contains(" api error")
        || s.contains("exception")
}

/// Agent 消息的语义分类——分布式 progress 通过前缀表达（见
/// `daemon.rs::progress_chunk_to_event_kind`），本地门客也会有失败文本落入
/// Error。渲染器据此选 rail/color/anchor。
///
/// 不改 `DialogueLine::Agent` 结构——所有分类在渲染时 pure-fn 派生。每帧重算
/// 便宜（只扫前缀），避免了事件入口处 classify、struct 扩字段带来的跨文件波及。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentKind {
    Assistant,
    Thinking,
    Tool,
    Error,
}

/// 剥离 `[thinking]` / `[tool]` / `[error]` / `[final]` 前缀并归类。
/// 返回引用避免 clone——调用方如需拥有所有权再 `to_string()`。
fn classify_agent_text(text: &str) -> (AgentKind, &str) {
    if let Some(rest) = text.strip_prefix("[thinking] ") {
        return (AgentKind::Thinking, rest);
    }
    if let Some(rest) = text.strip_prefix("[tool] ") {
        return (AgentKind::Tool, rest);
    }
    if let Some(rest) = text.strip_prefix("[error] ") {
        return (AgentKind::Error, rest);
    }
    if let Some(rest) = text.strip_prefix("[final] ") {
        // `[final]` 是 dist 失败 path 的补充摘要——语义上是 error 的尾巴，
        // 跟它同 rail 聚合比单独一类视觉更清爽。
        return (AgentKind::Error, rest);
    }
    if is_agent_error_text(text) {
        return (AgentKind::Error, text);
    }
    (AgentKind::Assistant, text)
}

/// 一 kind 一套视觉参数。
///
/// 设计：所有 kind 都走 `│` (U+2502) 续行 rail——单字符 + semantic color 区分
/// （对齐 opencode：靠颜色分类、不靠字符跳动）。`Assistant` 的 rail 用 muted
/// 色 = 低存在感；`Tool` / `Error` rail 用语义色让块边界更明显。
struct AgentStyle {
    anchor: &'static str,
    anchor_color: Color,
    body: Style,
    rail_color: Color,
    bold_first: bool,
}

fn style_for_agent_kind(kind: AgentKind, th: &Theme) -> AgentStyle {
    match kind {
        AgentKind::Assistant => AgentStyle {
            anchor: "● ",
            anchor_color: th.agent_first_line(),
            body: Style::default().fg(th.subtext1),
            rail_color: th.muted(),
            bold_first: true,
        },
        AgentKind::Thinking => AgentStyle {
            anchor: "◦ ",
            anchor_color: th.muted(),
            body: Style::default()
                .fg(th.muted())
                .add_modifier(Modifier::ITALIC),
            rail_color: th.muted(),
            bold_first: false,
        },
        AgentKind::Tool => AgentStyle {
            anchor: "▸ ",
            anchor_color: th.tool_call(),
            body: Style::default().fg(th.subtext1),
            rail_color: th.tool_call(),
            bold_first: false,
        },
        AgentKind::Error => AgentStyle {
            anchor: "✕ ",
            anchor_color: th.error(),
            body: Style::default().fg(th.error()),
            rail_color: th.error(),
            bold_first: false,
        },
    }
}

/// 硬截多行文本到 `max` 行。溢出时尾部替换为 `…` 单行（照抄 opencode 的
/// "不折叠不展开" 设计——长 tool 输出裸露太多不如直接砍）。
fn truncate_lines(text: &str, max: usize) -> String {
    let mut lines = text.lines();
    let mut kept: Vec<&str> = lines.by_ref().take(max).collect();
    if lines.next().is_some() {
        // 还有剩余——追加省略标记
        kept.push("…");
    }
    kept.join("\n")
}

/// 把多行 `text` 渲染为「首行锚点 + 续行缩进」的若干 `Line`。
///
/// - `anchor`: 首行前缀（`› ` / `● ` 等）
/// - `anchor_color`: 前缀色
/// - `bold`: agent 首行加粗（品牌色更醒目）；user 不加粗
/// - `body_style`: 正文 Span 的基础 Style（目前默认；留给将来染色接口）
fn push_anchored<'a>(
    out: &mut Vec<Line<'a>>,
    text: &str,
    anchor: &str,
    anchor_color: Color,
    bold: bool,
    body_style: Style,
) {
    push_anchored_with_rail(out, text, anchor, anchor_color, bold, body_style, None);
}

/// 已预渲染的 `Vec<Line>`（比如 markdown 处理过的 Assistant 文本）前缀锚点 / rail。
///
/// 等同 `push_anchored_with_rail` 的"行级"版本——给首行 Span 前插 anchor、
/// 其余行前插 rail。用 `std::mem::take` 搬迁 span ownership，`'static` 要求
/// 由上游 md_to_lines 返回 `Vec<Line<'static>>` 保证。
fn push_lines_with_rail(
    out: &mut Vec<Line<'static>>,
    mut lines: Vec<Line<'static>>,
    anchor: &str,
    anchor_color: Color,
    bold_first: bool,
    rail: Option<(char, Color)>,
) {
    if lines.is_empty() {
        // 空 markdown（比如纯空白输入）仍要挂一个 anchor 表示 "这条消息存在"
        out.push(Line::from(vec![
            Span::styled(
                anchor.to_string(),
                make_anchor_style(anchor_color, bold_first),
            ),
            Span::raw(String::new()),
        ]));
        return;
    }
    let rail_span = |r: Option<(char, Color)>| -> Span<'static> {
        match r {
            Some((ch, color)) => Span::styled(format!("{ch} "), Style::default().fg(color)),
            None => Span::raw(" ".repeat(ANCHOR_WIDTH)),
        }
    };
    for (idx, line) in lines.iter_mut().enumerate() {
        let existing = std::mem::take(&mut line.spans);
        let mut new_spans: Vec<Span<'static>> = Vec::with_capacity(existing.len() + 1);
        if idx == 0 {
            new_spans.push(Span::styled(
                anchor.to_string(),
                make_anchor_style(anchor_color, bold_first),
            ));
        } else {
            new_spans.push(rail_span(rail));
        }
        new_spans.extend(existing);
        line.spans = new_spans;
    }
    out.extend(lines);
}

fn make_anchor_style(color: Color, bold: bool) -> Style {
    let mut s = Style::default().fg(color);
    if bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    s
}

/// 扩展版：续行前缀可替换成彩色 rail（`│ ` + color）做视觉聚合。
///
/// `rail = None` 保持老行为——续行缩进两空格。`Some((ch, color))` 让每条
/// 续行首列画一个 rail 字符（典型 `│` U+2502），后跟空格，共 2 列保持与 anchor
/// 等宽。char 宽度必须是 1（box-drawing 范围内）——该函数内不做宽度检查，
/// 传入方保证。
fn push_anchored_with_rail<'a>(
    out: &mut Vec<Line<'a>>,
    text: &str,
    anchor: &str,
    anchor_color: Color,
    bold: bool,
    body_style: Style,
    rail: Option<(char, Color)>,
) {
    let anchor_text = anchor.to_string();
    let anchor_style = {
        let mut s = Style::default().fg(anchor_color);
        if bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        s
    };
    let continuation_span = |rail: Option<(char, Color)>| -> Span<'a> {
        match rail {
            Some((ch, color)) => Span::styled(format!("{ch} "), Style::default().fg(color)),
            None => Span::raw(" ".repeat(ANCHOR_WIDTH)),
        }
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
                continuation_span(rail),
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

fn collect_wrapped_plain_rows(lines: &[Line<'_>], width: u16) -> Vec<String> {
    let w = width.max(1) as usize;
    let mut out = Vec::new();
    for line in lines {
        let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for ch in plain.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
            if cur_w + cw > w && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push(ch);
            cur_w += cw;
        }
        if cur.is_empty() {
            out.push(String::new());
        } else {
            out.push(cur);
        }
    }
    out
}

fn slice_by_display_cols(s: &str, start_col: usize, end_col_exclusive: usize) -> String {
    if end_col_exclusive <= start_col {
        return String::new();
    }
    let mut out = String::new();
    let mut x = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        let next = x + cw;
        if next <= start_col {
            x = next;
            continue;
        }
        if x >= end_col_exclusive {
            break;
        }
        out.push(ch);
        x = next;
    }
    out.trim_end().to_string()
}

/// 把选中 cells 反色——给 draw 完的对话 buffer 叠 REVERSED modifier。
///
/// 选区语义：按列范围高亮（首尾行部分，中间行全宽），匹配区域复制心智模型。
/// `anchor`/`cursor` 是 Down/Drag 的 cell 坐标；被 clamp 到 `area`。
fn apply_selection_reverse(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    anchor: (u16, u16),
    cursor: (u16, u16),
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let inner_x = area.x;
    let inner_y = area.y;
    let inner_w = area.width;
    let inner_h = area.height;

    let clamp_x = |x: u16| -> u16 {
        x.max(inner_x)
            .min(inner_x.saturating_add(inner_w).saturating_sub(1))
    };
    let clamp_y = |y: u16| -> u16 {
        y.max(inner_y)
            .min(inner_y.saturating_add(inner_h).saturating_sub(1))
    };

    let a = (clamp_x(anchor.0), clamp_y(anchor.1));
    let b = (clamp_x(cursor.0), clamp_y(cursor.1));
    let ((sx, sy), (ex, ey)) = if (a.1, a.0) <= (b.1, b.0) {
        (a, b)
    } else {
        (b, a)
    };

    for y in sy..=ey {
        let row_start = if y == sy { sx } else { inner_x };
        let row_end = if y == ey {
            ex
        } else {
            inner_x.saturating_add(inner_w).saturating_sub(1)
        };
        if row_start > row_end {
            continue;
        }
        for x in row_start..=row_end {
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

#[derive(Debug, Clone)]
struct CollapsedEventRow {
    time: String,
    who: String,
    icon: &'static str,
    color: Color,
    narrative: String,
}

fn collapsible_tool_family_success(summary: &str) -> Option<&'static str> {
    let lower = summary.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("fail") {
        return None;
    }
    if lower.contains("read") {
        return Some("read");
    }
    if lower.contains("grep") {
        return Some("grep");
    }
    if lower.contains("glob") {
        return Some("glob");
    }
    None
}

fn collapse_consecutive_tools(rows: &[&EventRow]) -> Vec<CollapsedEventRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0usize;
    while i < rows.len() {
        let r = rows[i];
        let fam = if r.kind_tag == "tool_call_finished" {
            collapsible_tool_family_success(&r.summary)
        } else {
            None
        };
        if let Some(tool_family) = fam {
            let mut j = i + 1;
            let mut summaries = vec![r.summary.clone()];
            while j < rows.len() {
                let n = rows[j];
                let nfam = if n.kind_tag == "tool_call_finished" {
                    collapsible_tool_family_success(&n.summary)
                } else {
                    None
                };
                if nfam == Some(tool_family) {
                    summaries.push(n.summary.clone());
                    j += 1;
                } else {
                    break;
                }
            }
            let narrative = if summaries.len() <= 1 {
                narrate_event(r).2
            } else if summaries.len() == 2 {
                format!("工具完 · {} · {}", summaries[0], summaries[1])
            } else {
                format!(
                    "工具完 · {} · {} (+{} more)",
                    summaries[0],
                    summaries[1],
                    summaries.len() - 2
                )
            };
            let (icon, color, _) = narrate_event(r);
            out.push(CollapsedEventRow {
                time: r.time.clone(),
                who: r.who.clone(),
                icon,
                color,
                narrative,
            });
            i = j;
            continue;
        }

        let (icon, color, narrative) = narrate_event(r);
        out.push(CollapsedEventRow {
            time: r.time.clone(),
            who: r.who.clone(),
            icon,
            color,
            narrative,
        });
        i += 1;
    }
    out
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

fn humanize_tool_name(tool: &str) -> String {
    if tool.starts_with("tooluse_") {
        return "工具调用".to_string();
    }
    tool.to_string()
}

fn summarize_tool_preview(preview: &str) -> String {
    let one_line = preview.lines().next().unwrap_or(preview).trim();
    if one_line.starts_with('{')
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(one_line)
        && let Some(obj) = v.as_object()
    {
        let mut parts = Vec::new();
        for key in ["tool", "task_id", "agent_id", "status"] {
            if let Some(val) = obj.get(key).and_then(|x| x.as_str()) {
                parts.push(format!("{key}={}", truncate_by_width(val, 18)));
            }
        }
        if !parts.is_empty() {
            return parts.join(" ");
        }
    }
    truncate_by_width(one_line, 72)
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

#[cfg(test)]
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

fn humanize_elapsed_live(d: Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else {
        humanize_elapsed(d)
    }
}

/// γ `WorkerStatus` enum → 本地 TUI `NodeStatus` 的 1:1 映射。
/// 编译期保证完备：γ 加新变体时此处会编译失败，逼使 TUI 同步——比 string match
/// 加 fallback 更安全，team-lead review 拍板的方向。
fn node_status_from_enum(s: fuxi_core::WorkerStatus) -> NodeStatus {
    match s {
        fuxi_core::WorkerStatus::Alive => NodeStatus::Alive,
        fuxi_core::WorkerStatus::Stale => NodeStatus::Stale,
    }
}

/// 把 α `NodeSnapshot.status` 字符串解成本地 enum。
/// **wire 端仍是 String**（α/daemon 沿用，方便人肉看 JSON），所以 TUI 端要兜
/// 未知值——降级为 `Stale` + tracing warn：保守标红 + trace 留底，team-lead
/// review 要求"future 扩值不静默漏"。
fn decode_node_status(status: &str) -> NodeStatus {
    match status {
        "alive" => NodeStatus::Alive,
        "stale" => NodeStatus::Stale,
        "unknown" => NodeStatus::Unknown,
        other => {
            tracing::warn!(received = other, "未知 worker status 字符串，降级为 Stale");
            NodeStatus::Stale
        }
    }
}

async fn drive_tui(
    bus: EventBus,
    fuxi: Arc<Fuxi>,
    xuannv_id: AgentId,
    dist_ctrl: Option<Arc<crate::dist::DistController>>,
    resume_banner: Option<String>,
) -> Result<()> {
    if let Err(e) = redirect_stderr_to_log("/tmp/fuxi.log") {
        eprintln!("⚠ 无法重定向 stderr 到日志文件: {e}。TUI 可能被日志污染");
    }

    install_panic_hook();

    enable_raw_mode().context("enable_raw_mode")?;
    let mut stdout = io::stdout();
    // 参考 opencode：鼠标能力是静态配置项，不走运行时热切换。
    // 默认开启捕获（保证区域复制一致）；可用 `FUXI_ENABLE_MOUSE=0` 关闭并回终端原生选择。
    let mouse_capture_enabled = env_true_by_default("FUXI_ENABLE_MOUSE");
    // bracketed paste：让 IME / 剪贴板整块内容一次进入，不被 KEY_POLL 拆分成逐键序列
    if mouse_capture_enabled {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        )?;
    } else {
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    }
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
        // resume 提示用 toast，不污染 transcript。
        app.toasts.push(
            banner,
            crate::toast::ToastVariant::Info,
            Duration::from_secs(6),
        );
    }
    // 拓扑 panel 初始 snapshot——dist controller 启用时一次性拉全表灌进 ReplApp。
    // WHY 不轮询：公理 3，后续增量靠 EventBus WorkerRegistered/HeartbeatStateChanged/StaleSwept
    // 三事件推送。这里仅"开机第一帧 priming"，让 panel 不空。
    //
    // 顺序关键：**subscribe 必须在 snapshot 之前**，否则 [snapshot_at, subscribe_at]
    // 时间窗内发生的事件会丢——controller 已应用进它的 nodes 表（snapshot 拿到了），
    // 但 bus 上的事件没人订阅就被广播 drop，apply_snapshot 灌完后 TUI 永远比真实
    // 状态老一拍。
    //
    // 把订阅放在 snapshot 之前则相反：[subscribe_at, snapshot_at] 内的事件会被
    // broadcast 缓冲，主循环 select 第一轮就 drain 出来，叠在 snapshot 之上——
    // 三个 ingest handler 都是幂等/最新覆盖语义（upsert / overwrite inflight /
    // mark stale），重放无副作用。tokio broadcast 默认 cap 256，snapshot 量级
    // < 100ms 不会 lag。
    let mut stream = bus.subscribe();
    if let Some(ctrl) = &dist_ctrl {
        let snaps = ctrl.nodes_snapshot().await;
        app.apply_snapshot(snaps);
    }

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
                                    let send_text = app.expand_image_refs_for_submit(&text);
                                    tokio::spawn(async move {
                                        if let Err(e) = fuxi_cl.intervene(xuannv_id, false, &send_text).await {
                                            tracing::warn!(error = %e, "xuannv intervene 失败");
                                        }
                                    });
                                }
                                Some(Submit::Worker(id, text)) => {
                                    let mut meta = EventMeta::now();
                                    meta.agent = Some(id);
                                    meta.task = app.latest_task_id_for_worker(id);
                                    let _ = bus.publish(Event {
                                        meta,
                                        kind: EventKind::UserPrompted { text: text.clone() },
                                    });
                                    let fuxi_cl = fuxi.clone();
                                    let send_text = app.expand_image_refs_for_submit(&text);
                                    tokio::spawn(async move {
                                        if let Err(e) = fuxi_cl.intervene(id, false, &send_text).await {
                                            tracing::warn!(error = %e, "worker intervene 失败");
                                        }
                                    });
                                }
                                Some(Submit::Kill(id)) => {
                                    let fuxi_cl = fuxi.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = fuxi_cl.shutdown_agent(id, "user_kill".into()).await {
                                            tracing::warn!(error = %e, "worker kill 失败");
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
                            if mouse_capture_enabled {
                                app.handle_mouse(m);
                            }
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
        app.roles_by_agent
            .insert(card.id, card.profile.role.clone());
        if card.id == app.xuannv_id {
            app.xuannv_status = status;
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
            source_node_id: None,
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

    #[test]
    fn collapse_consecutive_tools_merges_same_family_success_rows() {
        let r1 = mk_row("tool_call_finished", "Read(path=a.rs)");
        let r2 = mk_row("tool_call_finished", "Read(path=b.rs)");
        let r3 = mk_row("tool_call_finished", "Read(path=c.rs)");
        let rows = vec![&r1, &r2, &r3];
        let out = collapse_consecutive_tools(&rows);
        assert_eq!(out.len(), 1, "连续同类 Read 应折成一行");
        assert!(out[0].narrative.contains("(+1 more)"));
    }

    #[test]
    fn collapse_consecutive_tools_keeps_failures_and_mixed_families() {
        let r1 = mk_row("tool_call_finished", "Read(path=a.rs)");
        let r2 = mk_row("tool_call_finished", "Read failed: permission denied");
        let r3 = mk_row("tool_call_finished", "Grep(query=foo)");
        let rows = vec![&r1, &r2, &r3];
        let out = collapse_consecutive_tools(&rows);
        assert_eq!(out.len(), 3, "失败行或跨族工具不应折叠");
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
        assert_eq!(app.ctrl_c_count, 1);
        app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
        assert_eq!(app.ctrl_c_count, 0);
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

    #[test]
    fn super_shortcuts_do_not_insert_text() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::SUPER);
        app.handle_key(KeyCode::Char('v'), KeyModifiers::SUPER);
        assert!(app.input_text().is_empty(), "Cmd/Ctrl 快捷键不应污染输入");
    }

    // ───────── 任务树：核心 Fix-D 断言 ─────────

    /// `TaskDispatched` 事件创建 task 节点，并沿用最近角色信息。
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
        assert_eq!(app.lookup_role(w), "dev");

        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        assert_eq!(app.tasks.len(), 1, "应有一个 task 节点");
        assert_eq!(app.tasks[0].worker, w);
        assert_eq!(app.tasks[0].worker_role, "dev");
    }

    #[test]
    fn same_task_id_can_attach_multiple_workers() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let a = AgentId::new();
        let b = AgentId::new();
        let tid = TaskId::new();

        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_ev(
            Some(b),
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: a },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: b },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskCreated {
                title: "跑全量测试".into(),
                description: "unit".into(),
            },
        ));

        let same_tid: Vec<_> = app.tasks.iter().filter(|t| t.task_id == tid).collect();
        assert_eq!(same_tid.len(), 2, "同 task_id 应能挂两个门客节点");
        assert!(same_tid.iter().any(|t| t.worker == a));
        assert!(same_tid.iter().any(|t| t.worker == b));
        assert!(same_tid.iter().all(|t| t.title == "跑全量测试"));
        assert!(same_tid.iter().all(|t| t.description == "unit"));
    }

    #[test]
    fn task_state_changed_targets_single_worker_when_agent_meta_present() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let a = AgentId::new();
        let b = AgentId::new();
        let tid = TaskId::new();

        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_ev(
            Some(b),
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: a },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: b },
        ));

        let mut done_ev = mk_task_ev(
            Some(a),
            tid,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        );
        done_ev.meta.agent = Some(a);
        app.ingest(&done_ev);

        let ta = app
            .tasks
            .iter()
            .find(|t| t.task_id == tid && t.worker == a)
            .expect("task a");
        let tb = app
            .tasks
            .iter()
            .find(|t| t.task_id == tid && t.worker == b)
            .expect("task b");
        assert_eq!(ta.state, TaskState::Done, "A 应被更新为 Done");
        assert_ne!(tb.state, TaskState::Done, "B 不应被 A 的事件连带完成");
    }

    /// Done 后仍保留在任务树，等待门客死亡再移除。
    #[test]
    fn task_done_keeps_node_until_worker_dead() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
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
        assert_eq!(app.tasks[0].state, TaskState::Done, "Done 应更新状态");
        // tick 不应删除 done 节点
        app.tick(Instant::now());
        assert_eq!(app.tasks.len(), 1, "done 节点应保留");
        app.ingest(&mk_ev(
            Some(w),
            EventKind::AgentDead {
                cause: "ws closed".into(),
            },
        ));
        assert!(app.tasks.is_empty(), "门客 dead 后应移除其任务节点");
    }

    #[test]
    fn latest_task_id_for_worker_prefers_non_terminal() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let w = AgentId::new();
        let tid_done = TaskId::new();
        let tid_active = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid_done,
            EventKind::TaskDispatched { to: w },
        ));
        app.ingest(&mk_task_ev(
            Some(w),
            tid_done,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid_active,
            EventKind::TaskDispatched { to: w },
        ));
        assert_eq!(app.latest_task_id_for_worker(w), Some(tid_active));
    }

    /// AgentSpawning 会登记 role（不再要求进入 idle 桶）。
    #[test]
    fn agent_spawning_records_role_mapping() {
        let mut app = ReplApp::stub();
        let w = AgentId::new();
        app.ingest(&mk_ev(
            Some(w),
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ));
        assert_eq!(app.lookup_role(w), "luban");
    }

    /// Tab 循环：只在任务门客间切换；Esc 回玄女。
    #[test]
    fn tab_cycles_only_workers_in_tasks() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let a = AgentId::new();
        let b = AgentId::new();
        // 两个门客都挂任务
        let tid = TaskId::new();
        let tid2 = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: a },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid2,
            EventKind::TaskDispatched { to: b },
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
            "Tab 2 应到 task 里的 b"
        );
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a), "Tab 3 循环回 a");
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

    fn toast_has_text(app: &ReplApp, needle: &str) -> bool {
        app.toasts.iter().any(|t| t.text.contains(needle))
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
            toast_has_text(&app, "再按一次 Esc"),
            "首按 Esc 应给 toast hint"
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
            toast_has_text(&app, "中断请求已发"),
            "二按应给中断确认 toast"
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
        assert!(!toast_has_text(&app, "中断请求已发"), "超窗不应发中断");
    }

    #[test]
    fn ctrl_c_exits() {
        let mut app = ReplApp::stub();
        // 首 Ctrl-C：打 confirm，不退。
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.should_quit, "单 Ctrl-C 不退");
        assert_eq!(app.ctrl_c_count, 1);

        // 紧接第二 Ctrl-C：退。
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit, "双 Ctrl-C 应退");
    }

    #[test]
    fn ctrl_c_timeout_requires_new_second_press() {
        let mut app = ReplApp::stub();
        let t0 = Instant::now();
        app.handle_key_at(KeyCode::Char('c'), KeyModifiers::CONTROL, t0);
        assert!(!app.should_quit, "首按不退出");

        // 超窗后再按，应视为新一轮首按，仍不退出。
        app.handle_key_at(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            t0 + Duration::from_millis(2500),
        );
        assert!(!app.should_quit, "超出双击窗口后第二次 Ctrl-C 不能直接退出");
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
            !toast_has_text(&app, "中断请求已发"),
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

    #[test]
    fn bracketed_paste_file_path_inserts_absolute_path() {
        let mut app = ReplApp::stub();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "hello").unwrap();

        app.handle_paste(&file.display().to_string());
        let got = app.input_text();
        assert!(
            got.contains(file.to_str().unwrap()),
            "粘贴文件路径应落绝对路径"
        );
    }

    #[test]
    fn bracketed_paste_image_path_inserts_image_ref() {
        let mut app = ReplApp::stub();
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("screen.png");
        std::fs::write(&img, b"not-real-png-but-path-exists").unwrap();

        app.handle_paste(&img.display().to_string());
        let got = app.input_text();
        assert!(
            got.contains("[image #1]"),
            "图片路径应转成 [image #n] 引用，实际: {got:?}"
        );
        assert!(
            !got.contains(img.to_str().unwrap()),
            "输入区不应展示绝对路径，实际: {got:?}"
        );
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(
            out,
            Some(Submit::Xuannv("[image #1]".into())),
            "提交到 transcript 的应保持 image ref"
        );
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
    fn auto_follow_uses_wrapped_row_total_not_logical_line_count() {
        let mut app = ReplApp::stub();
        for i in 0..10 {
            app.push_line(
                ActiveTarget::Xuannv,
                DialogueLine::Agent {
                    name: "玄女".into(),
                    text: format!(
                        "第{i}条：这是一个很长很长很长很长很长很长很长的消息，用来触发换行。"
                    ),
                },
            );
        }

        let backend = TestBackend::new(28, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();

        let bucket = app.dialogues.get(&ActiveTarget::Xuannv).unwrap();
        let logical_lines = render_dialogue_collapsed(bucket.iter()).len() as u16;
        assert!(
            app.last_dialogue_total > logical_lines,
            "总行数应按 wrap 后屏幕行计，而非逻辑行；total={}, logical={}",
            app.last_dialogue_total,
            logical_lines
        );
        assert_eq!(
            app.dialogue_scroll,
            app.last_dialogue_total
                .saturating_sub(app.last_dialogue_view),
            "auto-follow 下应始终贴底"
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
        // 模拟一次 draw 后的 area 状态——存的是「对话内容区」而不是含边框外框。
        // 当前对话块仅 TOP 分隔线，所以 content 区是 (x=28, y=1, w=60, h=19)。
        app.last_dialogue_area = Some(Rect::new(28, 1, 60, 19));
        let bucket = app.dialogues.get(&ActiveTarget::Xuannv).expect("bucket");
        let lines = render_dialogue_collapsed(bucket.iter());
        app.last_dialogue_wrapped_rows = collect_wrapped_plain_rows(&lines, 60);
        app.last_dialogue_view = 19;
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
        app.ingest(&mk_task_ev(
            Some(app.xuannv_id),
            TaskId::new(),
            EventKind::TaskDispatched { to: a },
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

        // 直接切回玄女——应还原 "hi"。
        app.switch_active(ActiveTarget::Xuannv);
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

        // 底部状态栏显示当前发送目标。
        let last = row_text(&buf, 23);
        let compact: String = last.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("你→玄女"),
            "底栏应含当前发送目标；实得: {last:?}"
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

        let all: String = (0..buf.area.height)
            .map(|y| row_text(&buf, y))
            .collect::<Vec<_>>()
            .join("\n");
        let compact: String = all.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            compact.contains("思考中")
                || compact.contains("衡量中")
                || compact.contains("推敲中")
                || compact.contains("分析中"),
            "busy 状态应显示在输入区附近；实得:\n{all}"
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

    /// 连续 agent 消息应被折叠为一个消息块：首行锚点，续行带 rail。
    ///
    /// Assistant kind 走 markdown，合并时用 `\n\n` 分隔成两段——段间多一个空行，
    /// 空行同样挂 rail（视觉上连续）。所以是 3 行：line1 / rail-only / line2。
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
        assert_eq!(rendered.len(), 3, "两段 + 段间空行 = 3 行");
        assert!(line_to_plain(&rendered[0]).starts_with("● "));
        assert!(
            line_to_plain(&rendered[1]).starts_with("│"),
            "段间空行应画 rail: {:?}",
            line_to_plain(&rendered[1])
        );
        assert!(
            line_to_plain(&rendered[2]).starts_with("│ "),
            "第二段应画 rail: {:?}",
            line_to_plain(&rendered[2])
        );
    }

    #[test]
    fn render_dialogue_merges_consecutive_agent_blocks() {
        let entries = [
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "玄女".into(),
                    text: "第一段".into(),
                },
                14,
                32,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "玄女".into(),
                    text: "第二段".into(),
                },
                14,
                33,
            ),
        ];
        let rendered = render_dialogue_collapsed(entries.iter());
        // Assistant markdown 两段合并用 `\n\n` 分隔 → 3 行（含段间空行 + rail）
        assert_eq!(rendered.len(), 3, "应合并成一块（两段 + 段间空行）");
        assert!(line_to_plain(&rendered[0]).starts_with("● "));
        assert!(
            line_to_plain(&rendered[2]).starts_with("│ "),
            "第二段应画 rail: {:?}",
            line_to_plain(&rendered[2])
        );
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
        assert!(line_to_plain(&rendered[0]).starts_with("› "));
        assert!(
            !line_to_plain(&rendered[1]).contains('›'),
            "第 2 段不应重复竖条: {:?}",
            line_to_plain(&rendered[1])
        );
        // User 走 no-rail（push_anchored 默认 None）——续行仍 2 空格。
        assert!(line_to_plain(&rendered[1]).starts_with("  "), "2 空格对齐");
        assert!(line_to_plain(&rendered[2]).starts_with("  "));
    }

    // ── 方向 2 · 分布式 progress 视觉分类 ──

    #[test]
    fn classify_agent_text_recognizes_dist_prefixes() {
        assert_eq!(
            classify_agent_text("[thinking] 递归扫描"),
            (AgentKind::Thinking, "递归扫描")
        );
        assert_eq!(
            classify_agent_text("[tool] $ ls -la"),
            (AgentKind::Tool, "$ ls -la")
        );
        assert_eq!(
            classify_agent_text("[error] enqueue failed"),
            (AgentKind::Error, "enqueue failed")
        );
        assert_eq!(
            classify_agent_text("[final] codex exited 2"),
            (AgentKind::Error, "codex exited 2"),
            "[final] 归 Error 类聚合到一条 rail"
        );
    }

    #[test]
    fn classify_agent_text_legacy_errors_go_to_error_kind() {
        // 老门客失败文本无前缀，`is_agent_error_text` 兜底。
        let (k, _) = classify_agent_text("API error: rate limit");
        assert_eq!(k, AgentKind::Error);
    }

    #[test]
    fn classify_agent_text_default_is_assistant() {
        let (k, s) = classify_agent_text("只是一段普通回复");
        assert_eq!(k, AgentKind::Assistant);
        assert_eq!(s, "只是一段普通回复");
    }

    /// thinking 前缀应走独立 rail，不和 assistant 合并（视觉上是两段）。
    #[test]
    fn render_dialogue_splits_assistant_and_thinking_kinds() {
        let entries = [
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "鲁班".into(),
                    text: "正在分析…".into(),
                },
                10,
                0,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "鲁班".into(),
                    text: "[thinking] 要递归三层".into(),
                },
                10,
                1,
            ),
        ];
        let rendered = render_dialogue_collapsed(entries.iter());
        // assistant 与 thinking 是两个独立块（各一行），无 kind 切换空行
        // （`dialogue_kind` 只区分到 DialogueLine variant，不进 agent sub-kind）。
        let plains: Vec<String> = rendered.iter().map(line_to_plain).collect();
        assert!(
            plains
                .iter()
                .any(|s| s.starts_with("● ") && s.contains("正在分析")),
            "应有 assistant anchor 且保留正文: {plains:?}"
        );
        // thinking 块的 anchor 是 "◦ "，内容不含 "[thinking]"（已剥离）
        let thinking_line = plains
            .iter()
            .find(|s| s.starts_with("◦ "))
            .expect("应出现 thinking anchor");
        assert!(
            thinking_line.contains("要递归三层"),
            "stripped content 应被保留: {thinking_line}"
        );
        assert!(
            !thinking_line.contains("[thinking]"),
            "前缀应被剥离: {thinking_line}"
        );
    }

    /// 同 kind 连续 chunks 要合并——分布式 worker 一轮 tool 会拆多条。
    #[test]
    fn render_dialogue_merges_consecutive_tool_chunks() {
        let entries = [
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "鲁班".into(),
                    text: "[tool] $ rg fn dispatch".into(),
                },
                10,
                0,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "鲁班".into(),
                    text: "[tool] src/daemon.rs:557".into(),
                },
                10,
                1,
            ),
        ];
        let rendered = render_dialogue_collapsed(entries.iter());
        let joined = rendered
            .iter()
            .map(line_to_plain)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("▸ $ rg fn dispatch"), "got:\n{joined}");
        assert!(joined.contains("│ src/daemon.rs:557"), "got:\n{joined}");
        assert!(!joined.contains("[tool]"), "前缀应被剥离: {joined}");
    }

    #[test]
    fn truncate_lines_caps_at_max_and_marks_overflow() {
        let s = "1\n2\n3\n4\n5";
        assert_eq!(truncate_lines(s, 10), "1\n2\n3\n4\n5", "未超上限原样返回");
        assert_eq!(truncate_lines(s, 3), "1\n2\n3\n…", "超出用 … 单行标记溢出");
    }

    #[test]
    fn render_dialogue_tool_body_truncated_to_ten_lines() {
        let long_tool = (1..=20)
            .map(|i| format!("[tool] line{i}"))
            .collect::<Vec<_>>();
        let entries: Vec<DialogueEntry> = long_tool
            .iter()
            .enumerate()
            .map(|(i, t)| {
                DialogueEntry::at_fixed(
                    DialogueLine::Agent {
                        name: "鲁班".into(),
                        text: t.clone(),
                    },
                    10,
                    i as u32 % 60,
                )
            })
            .collect();
        let rendered = render_dialogue_collapsed(entries.iter());
        // 10 条保留 + 1 条 "…" = 11 行；其他行（前缀空行等）不干扰断言方向
        let body_lines: Vec<String> = rendered.iter().map(line_to_plain).collect();
        assert!(
            body_lines
                .iter()
                .any(|l| l.trim_start_matches("│ ") == "…" || l.trim_start_matches("▸ ") == "…"),
            "应出现 … 溢出标记，实际: {body_lines:?}"
        );
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
        // 首行前缀按 unicode-width 应为 `› ` = 2 cells
        let first_prefix_width = UnicodeWidthStr::width("› ");
        assert_eq!(first_prefix_width, 2, "锚点宽度 = 2");
        // 续行 2 空格
        let second = line_to_plain(&rendered[1]);
        let leading_spaces: usize = second.chars().take_while(|c| *c == ' ').count();
        assert_eq!(leading_spaces, 2, "续行缩进 2 = 和锚点同宽");
    }

    /// 续行首字与首行首字同列——视觉"挂"在首行内容下方。
    ///
    /// Assistant 走 markdown——单 chunk 内 `\n` 是 soft break（变空格），
    /// 要用 `\n\n` 表达段落分隔。这里测 rail 宽度与 anchor 同宽，用多段文本。
    #[test]
    fn render_dialogue_v2_indent_alignment() {
        let entries = [DialogueEntry::at_fixed(
            DialogueLine::Agent {
                name: "".into(),
                text: "A\n\nB".into(),
            },
            12,
            0,
        )];
        let rendered = render_dialogue_collapsed(entries.iter());
        let first = line_to_plain(&rendered[0]);
        // A 段 + 空行（带 rail） + B 段——B 在第 3 行 (idx 2)
        let b_line = rendered
            .iter()
            .map(line_to_plain)
            .find(|s| s.contains('B'))
            .expect("应渲染 B 段");
        let first_content_col = UnicodeWidthStr::width(&first[..first.find('A').unwrap()]);
        let b_content_col = UnicodeWidthStr::width(&b_line[..b_line.find('B').unwrap()]);
        assert_eq!(
            first_content_col, b_content_col,
            "B 段首字应与 A 段首字同列——anchor `● ` 与 rail `│ ` 同宽"
        );
    }

    #[test]
    fn render_dialogue_collapses_consecutive_tool_lines() {
        let entries = [
            DialogueEntry::at_fixed(
                DialogueLine::Tool {
                    text: "Read(a.rs)".into(),
                    ok: true,
                },
                9,
                1,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Tool {
                    text: "Read(b.rs)".into(),
                    ok: true,
                },
                9,
                1,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Tool {
                    text: "Read(c.rs)".into(),
                    ok: true,
                },
                9,
                1,
            ),
        ];
        let rendered = render_dialogue_collapsed(entries.iter());
        assert_eq!(rendered.len(), 2, "连续工具消息应折叠成主行+次行");
        let plain = format!(
            "{}\n{}",
            line_to_plain(&rendered[0]),
            line_to_plain(&rendered[1])
        );
        assert!(plain.contains("Read(a.rs)"));
        assert!(plain.contains("Read(b.rs)"));
        assert!(plain.contains("(+1 more)"));
    }

    #[test]
    fn render_dialogue_does_not_merge_tool_with_non_tool() {
        let entries = [
            DialogueEntry::at_fixed(
                DialogueLine::Tool {
                    text: "Bash(cargo test)".into(),
                    ok: true,
                },
                9,
                2,
            ),
            DialogueEntry::at_fixed(
                DialogueLine::Agent {
                    name: "玄女".into(),
                    text: "测试完成".into(),
                },
                9,
                2,
            ),
        ];
        let rendered = render_dialogue_collapsed(entries.iter());
        assert_eq!(rendered.len(), 3, "两条 entry + 一条分隔空行");
        assert_eq!(line_to_plain(&rendered[1]), "");
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

    #[test]
    fn tooluse_finished_prefers_human_readable_started_label() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let worker = AgentId::new();
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({ "command": "cargo test -p fuxi-cli" }),
            },
        ));
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::ToolCallFinished {
                tool: "tooluse_xxx".into(),
                ok: true,
                output_preview: "".into(),
            },
        ));
        let w_bucket = app
            .dialogues
            .get(&ActiveTarget::Worker(worker))
            .cloned()
            .unwrap_or_default();
        let last = w_bucket.back().expect("one tool line");
        match &last.line {
            DialogueLine::Tool { text, ok } => {
                assert!(*ok);
                assert!(text.starts_with("Bash=cargo test"), "actual: {text}");
            }
            other => panic!("expected tool line, got {other:?}"),
        }
    }

    /// AgentDead 事件 → 该 worker 的任务立即移除。
    #[test]
    fn agent_dead_event_removes_worker_tasks() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
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
        let tid = TaskId::new();
        app.ingest(&mk_ev(
            Some(worker),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: worker },
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
            !compact.contains("空闲门客"),
            "任务树不应展示空闲门客:\n{all}"
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

        // '/' 作为第一个字符应当开 popup，且输入框可见 "/"
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        assert!(app.popup.is_open(), "空输入 + / 应开 popup");
        assert_eq!(app.input_text(), "/");
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

        // 键入 'h' → filter 变 "h"，候选收缩到 /help；输入框应同步显示 /h。
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        assert!(app.popup.is_open(), "popup 保持 open");
        assert_eq!(app.input_text(), "/h");
        assert_eq!(app.popup.display_input(), "/h");
        assert_eq!(app.popup.candidates().len(), 1);
        assert_eq!(app.popup.candidates()[0].slash, "/help");
    }

    #[test]
    fn popup_execute_routes_to_action() {
        // 从打开 popup → 键入过滤 → Enter → Action 被 run_command_action 路由。
        // 以 /help 为验证入口：run 后应打开 help overlay，不污染对话 bucket。
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty()); // /h → /help
        assert_eq!(app.popup.candidates()[0].slash, "/help");

        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out, None, "popup 的 Enter 返 None，不走 Submit 路径");
        assert!(!app.popup.is_open(), "Execute 后 popup 自闭合");
        assert!(app.help_overlay_open, "/help 应打开帮助面板");
        assert!(
            !app.dialogues.contains_key(&ActiveTarget::Xuannv),
            "/help 不应写入对话 transcript"
        );
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
    fn popup_enter_on_theme_does_not_execute_action() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty());
        assert_eq!(app.popup.candidates()[0].slash, "/theme");

        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert!(!app.popup.is_open());
        assert_eq!(app.input_text(), "/theme ");
        assert!(app.toasts.iter().next().is_none());
    }

    #[test]
    fn popup_enter_on_theme_completes_input_instead_of_execute() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty());
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(app.input_text(), "/theme ");
        assert!(
            app.toasts.iter().next().is_none(),
            "有参数命令 Enter 只补全，不执行"
        );
    }

    #[test]
    fn popup_tab_completes_input_without_execute() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty());
        app.handle_key(KeyCode::Tab, KeyModifiers::empty());
        assert_eq!(app.input_text(), "/theme");
        assert!(app.toasts.iter().next().is_none(), "Tab 只补全，不执行");
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
    fn try_handle_slash_help_opens_overlay_without_touching_dialogue() {
        let mut app = ReplApp::stub();
        let took = app.try_handle_slash_submit("/help");
        assert!(took, "/help 应被本地 handler 吃掉");
        assert!(app.help_overlay_open, "/help 应打开帮助面板");
        assert!(
            !app.dialogues.contains_key(&ActiveTarget::Xuannv),
            "/help 不应写入对话 transcript"
        );
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
    fn try_handle_slash_submit_unknown_command_shows_toast_and_consumes() {
        let mut app = ReplApp::stub();
        assert!(app.try_handle_slash_submit("/nope"));
        assert!(
            app.toasts.iter().any(|t| t.text.contains("未知命令 /nope")),
            "未知命令应给错误 toast"
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
        let tid = TaskId::new();
        app.ingest(&mk_ev(
            Some(a),
            EventKind::AgentSpawning {
                role: "dev".into(),
                cli: "cc".into(),
            },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: a },
        ));
        app.focus = Focus::Roster;
        app.roster_state.select(Some(1)); // 子门客行（0 是父任务头）
        app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(app.active, ActiveTarget::Worker(a));
        assert_eq!(app.focus, Focus::Input);
    }

    #[test]
    fn roster_enter_on_group_header_toggles_collapse() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let a = AgentId::new();
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: a },
        ));
        if let Some(t) = app.tasks.iter_mut().find(|t| t.task_id == tid) {
            t.title = "修 auth bug".into();
        }
        app.focus = Focus::Roster;
        app.roster_state.select(Some(0)); // 父任务头

        app.handle_key(KeyCode::Enter, KeyModifiers::empty()); // collapse
        let k = tid.to_string();
        assert!(app.collapsed_task_groups.contains(&k));
        assert_eq!(app.active, ActiveTarget::Xuannv, "折叠不应改 active");

        app.handle_key(KeyCode::Enter, KeyModifiers::empty()); // expand
        assert!(!app.collapsed_task_groups.contains(&k));
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
    fn slash_tree_toggles_sidebar_mode() {
        let mut app = ReplApp::stub();
        assert!(!app.tree_sidebar_enabled);
        assert!(app.try_handle_slash_submit("/tree"));
        assert!(app.tree_sidebar_enabled);
        assert!(app.try_handle_slash_submit("/tree"));
        assert!(!app.tree_sidebar_enabled);
    }

    #[test]
    fn slash_tree_accepts_on_off() {
        let mut app = ReplApp::stub();
        assert!(app.try_handle_slash_submit("/tree on"));
        assert!(app.tree_sidebar_enabled);
        assert!(app.try_handle_slash_submit("/tree off"));
        assert!(!app.tree_sidebar_enabled);
    }

    #[test]
    fn f4_focus_toggles_between_roster_and_input_in_sidebar_mode() {
        let mut app = ReplApp::stub();
        app.tree_sidebar_enabled = true;
        app.focus = Focus::Input;
        app.handle_key(KeyCode::F(4), KeyModifiers::empty());
        assert_eq!(app.focus, Focus::Roster);
        app.handle_key(KeyCode::F(4), KeyModifiers::empty());
        assert_eq!(app.focus, Focus::Input);
    }

    #[test]
    fn popup_shows_current_filter_text() {
        let mut app = ReplApp::stub();
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('z'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('z'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('z'), KeyModifiers::empty());
        assert!(app.popup.is_open());
        let backend = TestBackend::new(100, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut all = String::new();
        for y in 0..24 {
            all.push_str(&row_text(&buf, y));
            all.push('\n');
        }
        assert!(all.contains("/zzz"), "popup 应显示当前过滤串，实际:\n{all}");
    }

    #[test]
    fn teammate_task_tree_lines_are_task_rooted() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let w1 = AgentId::new();
        let w2 = AgentId::new();
        let tid1 = TaskId::new();
        let tid2 = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid1,
            EventKind::TaskDispatched { to: w1 },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            tid2,
            EventKind::TaskDispatched { to: w2 },
        ));
        if let Some(t) = app.tasks.iter_mut().find(|t| t.task_id == tid1) {
            t.title = "修 auth bug".to_string();
            t.worker_role = "鲁班".to_string();
        }
        if let Some(t) = app.tasks.iter_mut().find(|t| t.task_id == tid2) {
            t.title = "升级 rust 1.75".to_string();
            t.worker_role = "铸牒司".to_string();
        }
        let busy = app.busy_tasks();
        let lines = app.teammate_task_tree_lines(&busy);
        let merged = lines.join("\n");
        assert!(merged.contains("修 auth bug"));
        assert!(merged.contains("升级 rust 1.75"));
        assert!(merged.contains("└"));
        assert!(merged.contains("鲁班"));
        assert!(merged.contains("铸牒司"));
    }

    #[test]
    fn teammate_tree_separates_same_title_different_task_ids() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let w1 = AgentId::new();
        let w2 = AgentId::new();
        let t1 = TaskId::new();
        let t2 = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            t1,
            EventKind::TaskDispatched { to: w1 },
        ));
        app.ingest(&mk_task_ev(
            Some(xid),
            t2,
            EventKind::TaskDispatched { to: w2 },
        ));
        for t in app.tasks.iter_mut() {
            t.title = "跑全量测试".to_string();
            t.worker_role = "鲁班".to_string();
        }
        if let Some(t) = app.tasks.iter_mut().find(|x| x.task_id == t1) {
            t.description = "unit".to_string();
        }
        if let Some(t) = app.tasks.iter_mut().find(|x| x.task_id == t2) {
            t.description = "integ".to_string();
        }
        let busy = app.busy_tasks();
        let lines = app.teammate_task_tree_lines(&busy);
        let merged = lines.join("\n");
        let root_count = lines.iter().filter(|line| line.starts_with("▾ ")).count();
        assert_eq!(root_count, 2, "同标题不同 task_id 不应合并");
        assert!(merged.contains("跑全量测试"));
        assert!(merged.contains("鲁班"));
        assert!(!merged.contains("鲁班#2"), "跨任务不应追加 role #N");
        assert!(merged.contains("unit"));
        assert!(merged.contains("integ"));
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

    #[test]
    fn user_turn_like_titles_are_hidden_from_tree() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let w = AgentId::new();
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        if let Some(t) = app.tasks.iter_mut().find(|t| t.task_id == tid) {
            t.title = "user-turn 123".to_string();
        }
        assert!(
            app.visible_task_groups().is_empty(),
            "user-turn 变体不应进入任务树"
        );
    }

    #[test]
    fn duplicate_dispatch_same_task_worker_keeps_elapsed_anchor() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        let w = AgentId::new();
        let tid = TaskId::new();
        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        let anchored = Instant::now() - Duration::from_secs(5);
        if let Some(t) = app
            .tasks
            .iter_mut()
            .find(|t| t.task_id == tid && t.worker == w)
        {
            t.dispatched_at = anchored;
        }

        app.ingest(&mk_task_ev(
            Some(xid),
            tid,
            EventKind::TaskDispatched { to: w },
        ));
        let now_dispatched = app
            .tasks
            .iter()
            .find(|t| t.task_id == tid && t.worker == w)
            .expect("task exists")
            .dispatched_at;
        assert_eq!(now_dispatched, anchored, "重复派发不应重置 elapsed");
    }

    #[test]
    fn xuannv_agent_dead_clears_thinking_and_busy_since() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.xuannv_thinking = true;
        app.xuannv_busy_since = Some(Instant::now() - Duration::from_secs(2));
        app.ingest(&mk_ev(
            Some(xid),
            EventKind::AgentDead {
                cause: "test".to_string(),
            },
        ));
        assert!(!app.xuannv_thinking);
        assert!(app.xuannv_busy_since.is_none());
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

    // ───────── P6 拓扑 panel：4 条 ingest 行为契约 ─────────

    #[test]
    fn nodes_panel_renders_empty_state() {
        let app = ReplApp::stub();
        assert!(app.nodes.is_empty(), "新 ReplApp 不应有 node");
        assert_eq!(app.nodes_selected, 0);
        assert!(!app.nodes_overlay_open, "overlay 默认关闭");
    }

    #[test]
    fn nodes_panel_appends_on_worker_registered_event() {
        let mut app = ReplApp::stub();
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "home".into(),
                tags: vec!["cc".into(), "codex".into()],
                max_concurrency: 4,
            },
        ));
        assert_eq!(app.nodes.len(), 1);
        let n = &app.nodes[0];
        assert_eq!(n.node_id, "home");
        assert_eq!(n.max_concurrency, 4);
        assert_eq!(n.tags, vec!["cc".to_string(), "codex".to_string()]);
        assert_eq!(n.inflight, 0);
        // 仅 register 还未触达心跳——Unknown 比假装 Alive 诚实。
        assert_eq!(n.status, NodeStatus::Unknown);

        // 重连：再来一条 Registered 不重复插，仅更 tags/max。
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "home".into(),
                tags: vec!["cc".into()],
                max_concurrency: 8,
            },
        ));
        assert_eq!(app.nodes.len(), 1, "重连不应重复插");
        assert_eq!(app.nodes[0].max_concurrency, 8);
        assert_eq!(app.nodes[0].tags, vec!["cc".to_string()]);

        // 第二个节点按 node_id 排序插入。
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "alpha".into(),
                tags: vec![],
                max_concurrency: 1,
            },
        ));
        assert_eq!(app.nodes.len(), 2);
        assert_eq!(app.nodes[0].node_id, "alpha", "应按 id 排序");
        assert_eq!(app.nodes[1].node_id, "home");
    }

    #[test]
    fn nodes_panel_updates_inflight_on_heartbeat_state_changed_event() {
        let mut app = ReplApp::stub();
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "home".into(),
                tags: vec![],
                max_concurrency: 4,
            },
        ));
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerHeartbeatStateChanged {
                node_id: "home".into(),
                inflight_count: 2,
                status: fuxi_core::WorkerStatus::Alive,
            },
        ));
        assert_eq!(app.nodes[0].inflight, 2);
        assert_eq!(app.nodes[0].status, NodeStatus::Alive);

        app.ingest(&mk_ev(
            None,
            EventKind::WorkerHeartbeatStateChanged {
                node_id: "home".into(),
                inflight_count: 0,
                status: fuxi_core::WorkerStatus::Stale,
            },
        ));
        assert_eq!(app.nodes[0].inflight, 0);
        assert_eq!(app.nodes[0].status, NodeStatus::Stale);

        // 心跳来时无对应节点（早于 register）→ 静默忽略，不创节点。
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerHeartbeatStateChanged {
                node_id: "ghost".into(),
                inflight_count: 5,
                status: fuxi_core::WorkerStatus::Alive,
            },
        ));
        assert_eq!(app.nodes.len(), 1, "无 register 的心跳不该凭空建节点");
    }

    #[test]
    fn nodes_panel_apply_snapshot_replaces_all_nodes_sorted() {
        let mut app = ReplApp::stub();
        // 先放点旧 stale 数据，apply_snapshot 应整表替换不残留
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "old".into(),
                tags: vec![],
                max_concurrency: 1,
            },
        ));
        app.apply_snapshot(vec![
            crate::ipc::NodeSnapshot {
                node_id: "zulu".into(),
                tags: vec!["cc".into()],
                max_concurrency: 4,
                inflight_count: 2,
                inflight: vec!["j1".into(), "j2".into()],
                last_seen_ms_ago: Some(300),
                registered_at_ms_ago: Some(60_000),
                status: "alive".into(),
            },
            crate::ipc::NodeSnapshot {
                node_id: "alpha".into(),
                tags: vec![],
                max_concurrency: 1,
                inflight_count: 0,
                inflight: vec![],
                last_seen_ms_ago: Some(120_000),
                registered_at_ms_ago: Some(120_000),
                status: "stale".into(),
            },
        ]);
        assert_eq!(app.nodes.len(), 2, "snapshot 应整表替换");
        assert_eq!(app.nodes[0].node_id, "alpha", "应按 id 排序");
        assert_eq!(app.nodes[0].status, NodeStatus::Stale);
        assert_eq!(app.nodes[1].node_id, "zulu");
        assert_eq!(app.nodes[1].status, NodeStatus::Alive);
        assert_eq!(app.nodes[1].inflight, 2);
        assert_eq!(app.nodes[1].max_concurrency, 4);
        assert_eq!(app.nodes[1].tags, vec!["cc".to_string()]);
    }

    /// race-fix 合约：subscribe-then-snapshot 后，被 broadcast 缓冲的"过去事件"
    /// 重放叠在 snapshot 之上时，必须收敛到正确状态——三 ingest handler 必须
    /// 幂等/最新覆盖。这条测试钉死该不变式。
    ///
    /// 场景：snapshot 报 inflight=2 alive，但实际 controller 在 snapshot 之后
    /// 又收了一次心跳变 inflight=5 alive。subscribe 早于 snapshot 时，那条心跳
    /// 事件已在 broadcast 缓冲，主循环 drain 出来 ingest 后 inflight 必须是 5。
    #[test]
    fn nodes_panel_replay_event_after_snapshot_converges_to_latest() {
        let mut app = ReplApp::stub();
        // 1. snapshot 灌"过去状态" inflight=2
        app.apply_snapshot(vec![crate::ipc::NodeSnapshot {
            node_id: "home".into(),
            tags: vec!["cc".into()],
            max_concurrency: 8,
            inflight_count: 2,
            inflight: vec!["j1".into(), "j2".into()],
            last_seen_ms_ago: Some(500),
            registered_at_ms_ago: Some(60_000),
            status: "alive".into(),
        }]);
        assert_eq!(app.nodes[0].inflight, 2);

        // 2. 重放"晚于 snapshot 的真实事件"——主循环 drain broadcast 缓冲会做这事
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerHeartbeatStateChanged {
                node_id: "home".into(),
                inflight_count: 5,
                status: fuxi_core::WorkerStatus::Alive,
            },
        ));
        assert_eq!(
            app.nodes[0].inflight, 5,
            "重放 HSC 事件后 inflight 必须收敛到 5"
        );

        // 3. 验"重放 register 事件"幂等：snapshot 已记 home，再来一条 register
        // 不应重复插
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "home".into(),
                tags: vec!["cc".into(), "luban".into()],
                max_concurrency: 16,
            },
        ));
        assert_eq!(app.nodes.len(), 1, "重放 Registered 应幂等不重复插");
        assert_eq!(app.nodes[0].max_concurrency, 16);
        assert_eq!(
            app.nodes[0].tags,
            vec!["cc".to_string(), "luban".to_string()]
        );
        // 关键：register 不该重置 inflight=0——保持心跳带过来的 5
        assert_eq!(app.nodes[0].inflight, 5, "register 重连不应清 inflight");
    }

    /// `decode_node_status` 合约：已知值精确映射；未知值降级为 Stale（保守标红）
    /// 而非静默吞为 Unknown——团队约定：γ 加新 status 字符串时 TUI 必须能感知。
    #[test]
    fn decode_node_status_maps_known_and_falls_back_to_stale() {
        assert_eq!(decode_node_status("alive"), NodeStatus::Alive);
        assert_eq!(decode_node_status("stale"), NodeStatus::Stale);
        assert_eq!(decode_node_status("unknown"), NodeStatus::Unknown);
        // future-proofing：γ 加 "draining" / "rebalancing" 等新 status 时
        // TUI 不会假装"很好"——保守标 Stale 把眼神拉到这个 worker 上
        assert_eq!(decode_node_status("draining"), NodeStatus::Stale);
        assert_eq!(decode_node_status(""), NodeStatus::Stale);
    }

    #[test]
    fn nodes_panel_marks_stale_on_swept_event() {
        let mut app = ReplApp::stub();
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerRegistered {
                node_id: "laptop".into(),
                tags: vec![],
                max_concurrency: 2,
            },
        ));
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerHeartbeatStateChanged {
                node_id: "laptop".into(),
                inflight_count: 3,
                status: fuxi_core::WorkerStatus::Alive,
            },
        ));
        app.ingest(&mk_ev(
            None,
            EventKind::WorkerStaleSwept {
                node_id: "laptop".into(),
                recycled_jobs: vec!["j1".into(), "j2".into()],
            },
        ));
        assert_eq!(app.nodes[0].status, NodeStatus::Stale);
        assert_eq!(app.nodes[0].last_recycled_count, 2);
        // sweep 不清 inflight——dist.rs sweep_stale 行为；TUI 沿规约。
        assert_eq!(app.nodes[0].inflight, 3);
    }
}
