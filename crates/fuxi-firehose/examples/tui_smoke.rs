//! tui_smoke —— 本地冒烟用的最简闭环：内存 EventBus + 定时器合成事件 + TUI 驱动。
//!
//! 跑法：`cargo run -p fuxi-firehose --example tui_smoke`
//! 退出：按 `q`。
//!
//! 为什么是 example 而非 bin：CLI 层的正式入口在 `fuxi-cli`；这里只是给人眼验证用。

use crossterm::event::{self, Event as TermEvent};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use futures_util::StreamExt;
use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskId, TaskState};
use fuxi_events::EventBus;
use fuxi_firehose::FirehoseApp;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let bus = EventBus::with_memory_store().await?;
    let mut stream = bus.subscribe();

    // 后台 publisher：每 400ms 合成一条假事件。
    let pub_bus = bus.clone();
    tokio::spawn(async move {
        let agent = AgentId::new();
        let task = TaskId::new();
        let mut tick = tokio::time::interval(Duration::from_millis(400));
        let mut i = 0u64;
        loop {
            tick.tick().await;
            let ev = match i % 5 {
                0 => Event {
                    meta: with_agent(agent),
                    kind: EventKind::AgentReady {
                        endpoint: format!("http://127.0.0.1:{}/a2a", 6000 + (i as u16 % 10)),
                    },
                },
                1 => Event {
                    meta: with_agent_task(agent, task),
                    kind: EventKind::TaskCreated {
                        title: format!("task #{i}"),
                        description: "synthetic".into(),
                    },
                },
                2 => Event {
                    meta: with_agent_task(agent, task),
                    kind: EventKind::TaskStateChanged {
                        from: TaskState::New,
                        to: TaskState::Ready,
                    },
                },
                3 => Event {
                    meta: with_agent(agent),
                    kind: EventKind::ToolCallStarted {
                        tool: "echo".into(),
                        args: serde_json::json!({"i": i}),
                    },
                },
                _ => Event {
                    meta: with_agent(agent),
                    kind: EventKind::AgentResponded {
                        text: format!("synthetic response #{i}\nwith a newline"),
                        artifact_ref: None,
                    },
                },
            };
            if pub_bus.publish(ev).is_err() {
                break;
            }
            i += 1;
        }
    });

    // TUI 启动。
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = FirehoseApp::new();

    let res = run(&mut terminal, &mut app, &mut stream).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn with_agent(agent: AgentId) -> EventMeta {
    let mut m = EventMeta::now();
    m.agent = Some(agent);
    m
}

fn with_agent_task(agent: AgentId, task: TaskId) -> EventMeta {
    let mut m = EventMeta::now();
    m.agent = Some(agent);
    m.task = Some(task);
    m
}

async fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut FirehoseApp,
    stream: &mut fuxi_events::EventStream,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| app.draw(f))?;

        tokio::select! {
            maybe_ev = stream.next() => {
                match maybe_ev {
                    Some(Ok(ev)) => app.ingest(&ev),
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "event stream 错误");
                    }
                    None => break,
                }
            }
            // 把 crossterm 的阻塞 poll 塞到 blocking 线程里——按键延迟 50ms 没人会察觉。
            maybe_key = tokio::task::spawn_blocking(|| {
                if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                    match event::read() {
                        Ok(TermEvent::Key(k)) => Some(k.code),
                        _ => None,
                    }
                } else {
                    None
                }
            }) => {
                if let Ok(Some(code)) = maybe_key {
                    app.handle_key(code);
                    if app.should_quit() {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}
