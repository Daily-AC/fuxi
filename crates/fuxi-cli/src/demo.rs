//! `fuxi demo` —— 锚点场景的最小切片。
//!
//! 流程（P2 版本：走 orchestrator）：
//! 1. 本地起 `EventBus`（内存库）；
//! 2. 构造 `Fuxi` orchestrator（`allocate_worktree = false` 因为 demo 不假设
//!    cwd 是 git repo）；
//! 3. `fuxi.spawn_worker(profile, WorkerKind::Cc(cfg))` 拉一个 cc 门客——
//!    这一步会发 `AgentSpawning` / `AgentReady` 到 bus，同时 orchestrator
//!    维护生命周期；
//! 4. `fuxi.dispatch(id, task)` 把任务丢给门客——orchestrator 内置的 pump
//!    自动把门客 event 流 republish 到 bus；
//! 5. 主循环订阅 bus，stdout 或 TUI 渲染直到终结事件；
//! 6. `fuxi.shutdown()` 优雅关闭（发 AgentShuttingDown + AgentDead）。
//!
//! 公理体现：
//! - #1 显式沟通：stdout/TUI 看到的每条都是 agent 通过 A2A/Event 显式发出的；
//!   orchestrator 保证 lifecycle 事件闭合（Spawning→Ready→…→ShuttingDown→Dead）。
//! - #3 真实时：订阅是 push（broadcast），不是 poll；
//! - #5 SQLite 真相源：内存库也是 WAL，事件可回放（基建在那里）。

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::agent::AgentProfile;
use fuxi_core::event::EventKind;
use fuxi_core::task::{Task, TaskState};
use fuxi_events::EventBus;
use fuxi_firehose::FirehoseApp;
use fuxi_orchestrator::{Fuxi, FuxiConfig, WorkerKind};
use fuxi_workspace::GitWorktreeWorkspace;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::sync::Arc;
use std::time::Duration;

/// 把 demo 用到的各种字符串/开关集中。
#[derive(Debug, ClapArgs)]
pub struct Args {
    /// 给 cc 门客的 prompt。缺省值极短，便于快速验收。
    #[arg(default_value = "Reply with exactly: hi")]
    pub prompt: String,
    /// cc `--model`，覆盖 `FUXI_CC_MODEL` env。缺省走 `CcLaunchConfig::default`。
    #[arg(long)]
    pub model: Option<String>,
    /// 门客角色标签（写进 AgentProfile.role）。
    #[arg(long, default_value = "luban")]
    pub role: String,
    /// 门客名字（写进 AgentProfile.name）。
    #[arg(long, default_value = "cc-demo")]
    pub name: String,
    /// 启用 ratatui TUI——同时仍向 stderr 记日志，但 stdout 由 TUI 接管。
    #[arg(long)]
    pub tui: bool,
    /// 超时秒数——防止门客卡住时 demo 永不退出。
    #[arg(long, default_value = "120")]
    pub timeout: u64,
    /// 安静模式：过滤 cc 的 hook 噪声事件（`Custom { label: "cc_system_*" }`）。
    /// 默认 false（全火力输出），加了这个 flag 才眼睛友好。
    #[arg(long)]
    pub quiet: bool,
}

/// 是否应被 `--quiet` 过滤掉的"噪声"事件。
///
/// 当前只过 cc 的 hook/system 噪声；伏羲自身产生的平台事件 / agent 事件全保留。
fn is_noise(ev: &fuxi_core::Event) -> bool {
    matches!(
        &ev.kind,
        fuxi_core::EventKind::Custom { label, .. }
            if label.starts_with("cc_system_") || label == "rate_limit"
    )
}

pub async fn run(args: Args) -> Result<()> {
    // 1. 起 bus。
    let bus = EventBus::with_memory_store()
        .await
        .context("创建 EventBus 失败")?;

    // 2. 起 orchestrator（玄女）。demo 不假设 cwd 是 git repo，所以
    //    `allocate_worktree: false`——workspace 的构造只是为了占位。
    let cwd = std::env::current_dir().context("拿不到 cwd")?;
    let workspace = Arc::new(GitWorktreeWorkspace::with_default_base(cwd));
    let fuxi = Arc::new(Fuxi::with_config(
        bus.clone(),
        workspace,
        FuxiConfig {
            allocate_worktree: false,
            ..Default::default()
        },
    ));
    let _ = &args; // args 后面继续用。

    // 3. 构造 profile + launch cfg 交给 orchestrator。
    let mut cfg = CcLaunchConfig::default();
    if let Some(m) = args.model {
        cfg.model = Some(m);
    }
    let profile = AgentProfile {
        name: args.name.clone(),
        role: args.role.clone(),
        cli: "claude-code".to_string(),
        system_prompt: String::new(),
        tags: vec!["demo".to_string()],
        extra: Default::default(),
    };

    // 4. 让玄女拉门客——AgentSpawning + AgentReady 由 orchestrator 发。
    let agent_id = fuxi
        .spawn_worker(profile, WorkerKind::Cc(cfg))
        .await
        .context("玄女 spawn cc 门客失败")?;

    // 5. dispatch——事件流 republish 由 orchestrator 内置 pump 完成。
    let task = Task::new("demo", &args.prompt);
    tracing::info!(agent = %agent_id, task = %task.id, prompt = %args.prompt, "dispatch via fuxi");
    fuxi.dispatch(agent_id, task)
        .await
        .context("玄女 dispatch 失败")?;

    // 6. 输出回路——stdout stream 或 TUI 二选一。
    let outcome = if args.tui {
        drive_tui(bus.clone(), args.timeout, args.quiet).await
    } else {
        drive_stdout(bus.clone(), args.timeout, args.quiet).await
    };

    // 7. 清理——orchestrator.shutdown() 负责发 AgentShuttingDown + AgentDead。
    if let Err(e) = fuxi.shutdown().await {
        tracing::warn!(error = %e, "fuxi shutdown 异常（忽略）");
    }

    outcome
}

/// 纯 stdout 输出：每条事件打一行 JSON。
async fn drive_stdout(bus: EventBus, timeout_secs: u64, quiet: bool) -> Result<()> {
    let mut stream = bus.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                anyhow::bail!("demo 超时 ({timeout_secs}s) 仍未收到终结事件");
            }
            maybe = stream.next() => match maybe {
                Some(Ok(ev)) => {
                    // 终结事件即使在 quiet 模式也打印——方便判断 exit 原因。
                    let should_print = !quiet || !is_noise(&ev) || is_terminal(&ev);
                    if should_print {
                        println!("{}", serde_json::to_string(&ev)?);
                    }
                    if is_terminal(&ev) { return Ok(()); }
                }
                Some(Err(e)) => tracing::warn!(error = %e, "事件流错误"),
                None => return Ok(()),
            }
        }
    }
}

/// TUI 输出——和 `tui_smoke` 同一套骨架，但事件源是真 cc。
async fn drive_tui(bus: EventBus, timeout_secs: u64, quiet: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = FirehoseApp::new();
    let mut stream = bus.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut terminal_reached = false;

    let loop_res: Result<()> = async {
        loop {
            terminal.draw(|f| app.draw(f))?;
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    anyhow::bail!("demo 超时 ({timeout_secs}s) 仍未收到终结事件");
                }
                maybe_ev = stream.next() => match maybe_ev {
                    Some(Ok(ev)) => {
                        if is_terminal(&ev) { terminal_reached = true; }
                        if !quiet || !is_noise(&ev) || is_terminal(&ev) {
                            app.ingest(&ev);
                        }
                        if terminal_reached && app.should_quit() { return Ok(()); }
                    }
                    Some(Err(e)) => tracing::warn!(error = %e, "事件流错误"),
                    None => return Ok(()),
                },
                maybe_key = tokio::task::spawn_blocking(|| {
                    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                        match event::read() { Ok(TermEvent::Key(k)) => Some(k.code), _ => None }
                    } else { None }
                }) => {
                    if let Ok(Some(code)) = maybe_key {
                        app.handle_key(code);
                        if app.should_quit() { return Ok(()); }
                    }
                }
            }
        }
    }
    .await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    loop_res
}

/// 判定一条事件是否意味着 demo 该退出。
///
/// Blocked 保留为终结信号，因为 demo 里"阻塞"就是等不下去的同义词——
/// 和 orchestrator 内部"Blocked 可恢复"的语义是两个层次。
fn is_terminal(ev: &fuxi_core::Event) -> bool {
    matches!(
        ev.kind,
        EventKind::TaskStateChanged {
            to: TaskState::Done | TaskState::Cancelled | TaskState::Blocked,
            ..
        } | EventKind::AgentDead { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::{is_noise, is_terminal};
    use fuxi_core::event::{Event, EventKind, EventMeta};
    use fuxi_core::task::TaskState;

    fn ev(kind: EventKind) -> Event {
        Event {
            meta: EventMeta::now(),
            kind,
        }
    }

    #[test]
    fn terminal_detected() {
        assert!(is_terminal(&ev(EventKind::TaskStateChanged {
            from: TaskState::Delivering,
            to: TaskState::Done,
        })));
        assert!(is_terminal(&ev(EventKind::AgentDead {
            cause: "oom".into()
        })));
    }

    #[test]
    fn non_terminal_events_skip() {
        assert!(!is_terminal(&ev(EventKind::AgentReady {
            endpoint: "pid:42".into()
        })));
        assert!(!is_terminal(&ev(EventKind::AgentResponded {
            text: "hi".into()
        })));
        assert!(!is_terminal(&ev(EventKind::TaskStateChanged {
            from: TaskState::New,
            to: TaskState::Ready
        })));
    }

    #[test]
    fn noise_identified() {
        let noisy = EventKind::Custom {
            label: "cc_system_other".into(),
            payload: serde_json::json!({}),
        };
        assert!(is_noise(&ev(noisy)));
        let rate = EventKind::Custom {
            label: "rate_limit".into(),
            payload: serde_json::json!({}),
        };
        assert!(is_noise(&ev(rate)));
    }

    #[test]
    fn agent_events_are_not_noise() {
        assert!(!is_noise(&ev(EventKind::AgentResponded {
            text: "hi".into()
        })));
        assert!(!is_noise(&ev(EventKind::ThinkingStarted)));
        let custom_non_system = EventKind::Custom {
            label: "user_defined".into(),
            payload: serde_json::json!({}),
        };
        assert!(!is_noise(&ev(custom_non_system)));
    }
}
