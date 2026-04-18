//! `fuxi up` —— 平台长跑进程。
//!
//! P1 职责：
//! 1. 起 EventBus（可选 SQLite 文件路径，缺省内存）；
//! 2. 起 Firehose Hub，axum router 监听 `/ws` `/sse` `/events`；
//! 3. 发一条 `PlatformStarted` 事件做可观测的启动标记；
//! 4. 阻塞到 Ctrl-C；退出前发 `PlatformStopping`，优雅关闭。
//!
//! **尚未做（留给 P2）**：
//! - A2A server 接入（需要先有一个玄女角色实现 `A2AService`）；
//! - `fuxi spawn` 子命令——对 Up 进程发 A2A 请求要一个现成门客。

use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::{EventBus, EventStore};
use fuxi_firehose::Hub;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// HTTP 监听地址。
    #[arg(long, default_value = "127.0.0.1:4100")]
    pub bind: SocketAddr,
    /// 可选 SQLite 文件；不给则使用内存库（进程退出即丢）。
    #[arg(long)]
    pub db: Option<PathBuf>,
}

pub async fn run(args: Args) -> Result<()> {
    // 1. EventBus。
    let bus = match args.db.as_ref() {
        Some(path) => {
            let store = EventStore::connect_file(path)
                .await
                .with_context(|| format!("打开 SQLite 数据库 {}", path.display()))?;
            EventBus::new(store, Default::default())
        }
        None => EventBus::with_memory_store()
            .await
            .context("创建内存 EventBus")?,
    };

    // 2. Hub + router。
    let hub = Arc::new(Hub::new(bus.clone()));
    let app = fuxi_firehose::hub::router(hub);

    // 3. 标记启动。
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    })
    .ok();
    tracing::info!(addr = %args.bind, "伏羲 platform up; WS at /ws, SSE at /sse, REST at /events");
    eprintln!(
        "伏羲 up · listening on http://{}  (Ctrl-C to stop)",
        args.bind
    );

    // 4. 启动 axum。
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind {} 失败", args.bind))?;
    let shutdown = wait_for_shutdown();
    let serve_fut = axum::serve(listener, app).with_graceful_shutdown(shutdown);

    let result = serve_fut.await;

    // 5. 发 PlatformStopping 再退。
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStopping,
    })
    .ok();
    // 给 writer 一个极短窗口落库。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    result.context("axum serve 异常")
}

/// 等待 Ctrl-C。Unix 下同时捕 SIGTERM。
async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "无法监听 SIGTERM，仅 Ctrl-C");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("收到 Ctrl-C"),
            _ = sigterm.recv() => tracing::info!("收到 SIGTERM"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("收到 Ctrl-C");
    }
}
