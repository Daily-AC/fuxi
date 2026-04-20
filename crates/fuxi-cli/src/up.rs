//! `fuxi up` —— 平台长跑进程。
//!
//! v0.1 职责：
//! 1. 起 EventBus（可选 SQLite 文件路径，缺省内存）；
//! 2. 起 Fuxi orchestrator（玄女的本体）；
//! 3. 起 Firehose Hub，axum router 监听 `/ws` `/sse` `/events`；
//! 4. **起 daemon Unix socket listener**（`$FUXI_SOCK` 默认 `/tmp/fuxi.sock`）
//!    —— 接收玄女 CC 发来的 spawn/dispatch/intervene 等子命令；
//! 5. 发一条 `PlatformStarted` 事件做可观测的启动标记；
//! 6. 阻塞到 Ctrl-C 或 `fuxi` 收到 `Shutdown` 命令；退出前发 `PlatformStopping`。

use crate::daemon::Daemon;
use crate::ipc;
use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::{EventBus, EventStore};
use fuxi_firehose::Hub;
use fuxi_memory::OracleStore;
use fuxi_orchestrator::{Fuxi, FuxiConfig};
use fuxi_scheduler::keeper::SystemClock;
use fuxi_scheduler::watcher::{FsWatcherConfig, FsWatcherRig};
use fuxi_scheduler::webhook::WebhookState;
use fuxi_scheduler::{Keeper, TriggerStore};
use fuxi_workspace::GitWorktreeWorkspace;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// HTTP 监听地址（Firehose Hub）。
    #[arg(long, default_value = "127.0.0.1:4100")]
    pub bind: SocketAddr,
    /// 可选 SQLite 文件；不给则使用内存库（进程退出即丢）。
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// 可选 scheduler SQLite 文件——默认跟 `--db` 同位（若无则 `:memory:`）。
    #[arg(long = "sched-db")]
    pub sched_db: Option<PathBuf>,
    /// Unix socket 路径覆盖。默认走 `$FUXI_SOCK` 或 `/tmp/fuxi.sock`。
    #[arg(long = "sock")]
    pub sock_path: Option<PathBuf>,
    /// 工作区根（存 worktree 的地方）。默认当前目录。
    #[arg(long, default_value = ".")]
    pub workspace_root: PathBuf,
    /// 门客是否分配 worktree（demo 场景可以关掉）。
    #[arg(long, default_value_t = true)]
    pub allocate_worktree: bool,
}

pub async fn run(args: Args) -> Result<()> {
    // 0. M3.2 迁移：~/.fuxi/skills → ~/.fuxi/roles（幂等）。失败只 warn，不阻止启动。
    if let Err(e) = fuxi_skills::migrate_user_dir() {
        tracing::warn!(error = %e, "M3.2 用户目录迁移出错，忽略继续");
    }

    // 1. EventBus
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

    // 2. Workspace + Fuxi orchestrator
    let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
        args.workspace_root.clone(),
    ));
    let fuxi_cfg = FuxiConfig {
        allocate_worktree: args.allocate_worktree,
        ..Default::default()
    };
    let fuxi = Arc::new(Fuxi::with_config(bus.clone(), ws, fuxi_cfg));

    // 3. Scheduler（更漏）—— candidate 落库 + Keeper tick loop
    let sched_store = match args.sched_db.as_ref().or(args.db.as_ref()) {
        Some(path) => {
            // 为什么用独立文件 suffix：scheduler 和 events 共存同一文件没错，但分离后
            // 各自的 PRAGMA 设置不会相互踩到；若用户只给 --db，那就同一文件也能跑。
            TriggerStore::connect_file(path)
                .await
                .with_context(|| format!("打开 scheduler SQLite {}", path.display()))?
        }
        None => TriggerStore::connect_memory()
            .await
            .context("创建 scheduler 内存库")?,
    };
    let keeper = Arc::new(Keeper::new(
        sched_store.clone(),
        bus.clone(),
        Arc::new(SystemClock),
    ));
    let keeper_task = Arc::clone(&keeper).spawn();

    // fs_watch triggers 启动时一次性挂上
    let fs_rig = FsWatcherRig::spawn(
        sched_store.clone(),
        keeper.clone(),
        FsWatcherConfig::default(),
    )
    .await
    .context("启动 fs watcher")?;

    // 4. Firehose Hub + webhook router，合并成同一 app
    let hub = Arc::new(Hub::new(bus.clone()));
    let firehose_router = fuxi_firehose::hub::router(hub);
    let webhook_router = fuxi_scheduler::webhook::router(WebhookState {
        store: sched_store.clone(),
        keeper: keeper.clone(),
    });
    let app = firehose_router.merge(webhook_router);

    // 5. Daemon socket + 策府（P2 召回）
    //
    // 复用 `--db` 同一个 sqlite 文件——OracleStore 自己 init schema，幂等；和
    // EventStore 共存同一文件没问题（PRAGMA WAL 双方互不踩）。无 db 时走 :memory:
    // 兜底——repl.rs 也起了一个 OracleStore，但 up.rs 是独立 daemon 进程，自己起。
    let oracle = match args.db.as_ref() {
        Some(path) => OracleStore::connect_file(path)
            .await
            .with_context(|| format!("打开策府 SQLite {}", path.display()))?,
        None => OracleStore::connect_memory()
            .await
            .context("创建策府内存库")?,
    };
    let sock_path = args.sock_path.clone().unwrap_or_else(ipc::socket_path);
    // P2 召回入库钩子——daemon 持 oracle 是查询路径（--recall-task/role 时用），
    // sink 是写入路径（dispatch pump Done 时由 RecallContext 提供 role/worktree/sid）。
    // sink 不再持 fuxi 反查 role——L2 后 pump 直接送整包 context。
    fuxi.set_recall_sink(Arc::new(crate::recall_sink::OracleRecallSink::new(
        oracle.clone(),
    )))
    .await;
    let daemon = Daemon::new(
        fuxi.clone(),
        bus.clone(),
        sched_store.clone(),
        keeper.clone(),
        oracle,
    );
    let daemon_shutdown = daemon.shutdown_handle();
    let sock_for_task = sock_path.clone();
    let daemon_task = tokio::spawn(async move {
        if let Err(e) = daemon.serve(&sock_for_task).await {
            tracing::error!(error = %e, "daemon serve 异常退出");
        }
    });

    // 5. PlatformStarted
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    })
    .ok();
    tracing::info!(addr = %args.bind, sock = %sock_path.display(), "伏羲 platform up");
    eprintln!(
        "伏羲 up\n  HTTP  http://{}  (WS /ws · SSE /sse · REST /events)\n  SOCK  {} (玄女工具口)\n  Ctrl-C 停止",
        args.bind,
        sock_path.display()
    );

    // 6. 启 axum Hub，阻塞到 Ctrl-C 或 daemon shutdown
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind {} 失败", args.bind))?;
    let daemon_shutdown_for_axum = daemon_shutdown.clone();
    let shutdown = async move {
        tokio::select! {
            _ = wait_for_shutdown() => {}
            _ = daemon_shutdown_for_axum.notified() => {
                tracing::info!("daemon 收到 Shutdown 命令，停 axum");
            }
        }
    };
    let serve_fut = axum::serve(listener, app).with_graceful_shutdown(shutdown);
    let result = serve_fut.await;

    // 7. 通知 daemon 停（如果不是它先触发的）
    daemon_shutdown.notify_waiters();
    let _ = daemon_task.await;
    keeper_task.abort();
    fs_rig.join.abort();
    drop(fs_rig); // 确保 watchers 的生命周期结束

    // 8. PlatformStopping
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStopping,
    })
    .ok();
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
