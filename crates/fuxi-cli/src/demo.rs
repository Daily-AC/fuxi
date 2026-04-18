//! `fuxi demo` —— 锚点场景的最小切片。
//!
//! 流程：
//! 1. 本地起 `EventBus`（内存库）；
//! 2. 构造 `AgentProfile` + `CcLaunchConfig`，`CcAgent::launch` 起子进程；
//! 3. 让 bus 订阅一路打 stdout；若 `--tui` 再开一路驱动 `FirehoseApp`；
//! 4. `agent.dispatch(task)` 拿到 `mpsc::Receiver<Event>`，起一个 pump 任务
//!    把它的每条事件 republish 到 bus；
//! 5. 主循环等终结事件（`TaskStateChanged -> Done` 或 `TaskBlocked`/超时）；
//! 6. 优雅 shutdown。
//!
//! 公理体现：
//! - #1 显式沟通：stdout 打印的每条都是 agent 通过 A2A/Event 显式发出的；
//! - #3 真实时：订阅是 push（broadcast），不是 poll；
//! - #5 SQLite 真相源：内存库也是 WAL，事件可回放（demo 结束前用户未必查，但
//!   基建在那里）。

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use fuxi_agent_cc::{CcAgent, CcLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::task::{Task, TaskState};
use fuxi_events::EventBus;
use fuxi_firehose::FirehoseApp;
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
    #[arg(long, default_value = "dev")]
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
}

pub async fn run(args: Args) -> Result<()> {
    // 1. 起 bus。
    let bus = EventBus::with_memory_store()
        .await
        .context("创建 EventBus 失败")?;

    // 2. 构造 agent。
    let mut cfg = CcLaunchConfig::default();
    if let Some(m) = args.model {
        cfg.model = m;
    }
    let profile = AgentProfile {
        name: args.name.clone(),
        role: args.role.clone(),
        cli: "claude-code".to_string(),
        system_prompt: String::new(),
        tags: vec!["demo".to_string()],
        extra: Default::default(),
    };
    let agent = CcAgent::launch(profile.clone(), cfg).context("启动 cc 子进程失败")?;
    let agent_id = agent.card().id;
    let agent = Arc::new(agent);

    // 3. 发几条平台事件让视觉上"有头有尾"。
    let spawning = Event {
        meta: {
            let mut m = EventMeta::now();
            m.agent = Some(agent_id);
            m
        },
        kind: EventKind::AgentSpawning {
            role: profile.role.clone(),
            cli: "claude-code".into(),
        },
    };
    bus.publish(spawning).ok();

    // 4. dispatch。
    let task = Task::new("demo", &args.prompt);
    let task_id = task.id;
    tracing::info!(task = %task_id, prompt = %args.prompt, "dispatching to cc");

    // 发 TaskCreated。
    bus.publish(Event {
        meta: {
            let mut m = EventMeta::now();
            m.agent = Some(agent_id);
            m.task = Some(task_id);
            m
        },
        kind: EventKind::TaskCreated {
            title: task.title.clone(),
            description: task.description.clone(),
        },
    })
    .ok();
    bus.publish(Event {
        meta: {
            let mut m = EventMeta::now();
            m.agent = Some(agent_id);
            m.task = Some(task_id);
            m
        },
        kind: EventKind::TaskStateChanged {
            from: TaskState::New,
            to: TaskState::InProgress,
        },
    })
    .ok();

    // Dispatch 开始拿 event receiver。
    let mut rx = agent
        .dispatch(task)
        .await
        .map_err(|e| anyhow::anyhow!("cc dispatch 失败：{e}"))?;

    // 5. Pump：把 agent 的 events republish 到 bus。
    let pump_bus = bus.clone();
    let pump_task = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if pump_bus.publish(ev).is_err() {
                break;
            }
        }
    });

    // 6. 输出回路——stdout stream 或 TUI 二选一。
    let outcome = if args.tui {
        drive_tui(bus.clone(), args.timeout).await
    } else {
        drive_stdout(bus.clone(), args.timeout).await
    };

    // 7. 清理。
    pump_task.abort();
    if let Err(e) = agent.shutdown().await {
        tracing::warn!(error = %e, "agent shutdown 异常（忽略）");
    }

    // 8. 收尾事件。
    bus.publish(Event {
        meta: {
            let mut m = EventMeta::now();
            m.agent = Some(agent_id);
            m
        },
        kind: EventKind::AgentShuttingDown {
            reason: "demo-end".into(),
        },
    })
    .ok();

    outcome
}

/// 纯 stdout 输出：每条事件打一行 JSON。
async fn drive_stdout(bus: EventBus, timeout_secs: u64) -> Result<()> {
    let mut stream = bus.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                anyhow::bail!("demo 超时 ({timeout_secs}s) 仍未收到终结事件");
            }
            maybe = stream.next() => match maybe {
                Some(Ok(ev)) => {
                    println!("{}", serde_json::to_string(&ev)?);
                    if is_terminal(&ev) { return Ok(()); }
                }
                Some(Err(e)) => tracing::warn!(error = %e, "事件流错误"),
                None => return Ok(()),
            }
        }
    }
}

/// TUI 输出——和 `tui_smoke` 同一套骨架，但事件源是真 cc。
async fn drive_tui(bus: EventBus, timeout_secs: u64) -> Result<()> {
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
                        app.ingest(&ev);
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
fn is_terminal(ev: &Event) -> bool {
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
    use super::is_terminal;
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
}
