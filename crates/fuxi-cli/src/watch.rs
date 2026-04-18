//! `fuxi watch` —— 连上运行中的 Hub，用 ratatui TUI 观察事件。
//!
//! 与 `demo --tui` 的区别：`demo` 的事件源是本地 bus（内存，同进程）；
//! `watch` 的事件源是远端 Hub 的 WebSocket。两者共用同一个 `FirehoseApp`。

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use fuxi_firehose::{FirehoseApp, connect_ws};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::time::Duration;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// 远端 Hub 的 WebSocket URL。
    #[arg(long, default_value = "ws://127.0.0.1:4100/ws")]
    pub url: String,
    /// 可选：从此 event_id 起回放再接 live。
    #[arg(long)]
    pub from_id: Option<String>,
}

pub async fn run(args: Args) -> Result<()> {
    // 拼 URL：若给了 from_id 追加到 query。
    let url_str = match &args.from_id {
        Some(id) if args.url.contains('?') => format!("{}&from_id={id}", args.url),
        Some(id) => format!("{}?from_id={id}", args.url),
        None => args.url.clone(),
    };
    let url = url::Url::parse(&url_str).with_context(|| format!("解析 WS URL: {url_str}"))?;

    let mut stream = connect_ws(url).await.context("连接 Hub WebSocket")?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = FirehoseApp::new();

    let loop_res: Result<()> = async {
        loop {
            terminal.draw(|f| app.draw(f))?;
            tokio::select! {
                maybe_ev = stream.next() => match maybe_ev {
                    Some(Ok(ev)) => app.ingest(&ev),
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
