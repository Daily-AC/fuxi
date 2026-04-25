//! `fuxi im start` —— IM 后端长跑进程（家用部署的入口）。
//!
//! 设计取舍：home 上 systemd 跑这一个进程就是完整伏羲——它**复用 `up.rs` 的核心
//! wiring**（EventBus + Fuxi + Scheduler + daemon socket）+ 加 fuxi-im axum on
//! `:9100`，外加 ε 的 PWA `dist/` 用 ServeDir 挂 `/`。
//!
//! 为什么不另起一个独立 IM 进程：`EventBus` 是进程内 `tokio::broadcast`，跨进程
//! 不可能共享；`fuxi-im` 的 push hooks 必须订阅同一个 bus 才能感知玄女 idle /
//! task done。所以 home 只能一个进程。
//!
//! 跟 `up.rs` 的差别：
//! - 不挂 firehose hub（手机端走 IM WS，不需要 firehose 路由）
//! - 不挂 dist controller（家用机自己也不是 dist controller）
//! - 多挂 IM router（含静态 PWA + token 鉴权 + push 钩子）
//! - 默认 bind `:9100`（nginx 反代到此），不是 `:4100`
//!
//! 部署预期路径（systemd unit 里写死）：
//! - 二进制：`/home/e0-7/.local/bin/fuxi`
//! - 数据库：`~/.fuxi/events.db`（事件库） + `~/.fuxi/im.db`（β/δ 持久层）
//! - PWA dist：`/home/e0-7/.local/share/fuxi/im-web/`（install.sh 推到这）
//! - HMAC / VAPID：`~/.fuxi/im_hmac.key` / `~/.fuxi/im_vapid.json`（首启自生）

use crate::daemon::Daemon;
use crate::ipc;
use anyhow::{Context, Result};
use clap::Args as ClapArgs;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::{EventBus, EventStore};
use fuxi_im::auth::HmacSecret;
use fuxi_im::db as im_db;
use fuxi_im::devices::DeviceStore;
use fuxi_im::push::notify::HyperPushSender;
use fuxi_im::state::{AppState, ImAuth, ImPush};
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
use tower_http::services::ServeDir;

#[derive(Debug, ClapArgs)]
pub struct StartArgs {
    /// IM HTTP 监听地址。默认 `127.0.0.1:9100`——nginx 反代到此。
    #[arg(long, default_value = "127.0.0.1:9100")]
    pub bind: SocketAddr,
    /// EventBus SQLite 路径。默认 `$HOME/.fuxi/events.db`；不存在则建。
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// Scheduler SQLite。默认与 `--db` 同位。
    #[arg(long = "sched-db")]
    pub sched_db: Option<PathBuf>,
    /// daemon Unix socket 路径。默认 `$FUXI_SOCK` 或 `/tmp/fuxi.sock`。
    #[arg(long = "sock")]
    pub sock_path: Option<PathBuf>,
    /// 工作区根（worktree 落地处）。默认 `$HOME/fuxi-workspace`。
    #[arg(long)]
    pub workspace_root: Option<PathBuf>,
    /// PWA dist 目录。默认 `$HOME/.local/share/fuxi/im-web`（install.sh 推到此）。
    #[arg(long = "web-root")]
    pub web_root: Option<PathBuf>,
    /// 是否给门客分配 worktree。默认开。
    #[arg(long, default_value_t = true)]
    pub allocate_worktree: bool,
}

pub async fn run(args: StartArgs) -> Result<()> {
    // 0. M3.2 迁移（与 up.rs 同款 best-effort）
    if let Err(e) = fuxi_skills::migrate_user_dir() {
        tracing::warn!(error = %e, "M3.2 用户目录迁移出错，忽略继续");
    }

    // 1. EventBus —— 默认 $HOME/.fuxi/events.db。home 是 systemd 长跑机器，
    //    内存库重启即丢，不可接受；强制走文件库（无 $HOME 才报错退出）。
    //    EventStore::connect_file 用 sqlx create_if_missing 建文件但**不**建
    //    父目录——首启时 ~/.fuxi/ 不存在直接 (code: 14) unable to open database
    //    file。我们这里主动 mkdir -p（im_db::init_at 也是同样自卫做法）。
    let events_db_path = args
        .db
        .clone()
        .or_else(default_events_db_path)
        .ok_or_else(|| anyhow::anyhow!("无法解析 events.db 路径：$HOME 未设置且未传 --db"))?;
    if let Some(parent) = events_db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建事件库父目录: {}", parent.display()))?;
    }
    let store = EventStore::connect_file(&events_db_path)
        .await
        .with_context(|| format!("打开事件库 {}", events_db_path.display()))?;
    let bus = EventBus::new(store, Default::default());

    // 2. Workspace + Fuxi
    let workspace_root = args
        .workspace_root
        .clone()
        .or_else(default_workspace_root)
        .ok_or_else(|| {
            anyhow::anyhow!("无法解析 workspace_root：$HOME 未设置且未传 --workspace-root")
        })?;
    std::fs::create_dir_all(&workspace_root)
        .with_context(|| format!("workspace_root 不可创建: {}", workspace_root.display()))?;
    let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
        workspace_root.clone(),
    ));
    let fuxi_cfg = FuxiConfig {
        allocate_worktree: args.allocate_worktree,
        ..Default::default()
    };
    let fuxi = Arc::new(Fuxi::with_config(bus.clone(), ws, fuxi_cfg));

    // 3. Scheduler（更漏）—— 与 up.rs 一致
    let sched_store = match args.sched_db.as_ref().or(Some(&events_db_path)) {
        Some(path) => TriggerStore::connect_file(path)
            .await
            .with_context(|| format!("打开 scheduler SQLite {}", path.display()))?,
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
    let fs_rig = FsWatcherRig::spawn(
        sched_store.clone(),
        keeper.clone(),
        FsWatcherConfig::default(),
    )
    .await
    .context("启动 fs watcher")?;

    // 4. IM AppState wiring：
    //    - im_auth: HmacSecret::load_or_create_default + DeviceStore on im.db
    //    - im_push: VapidKeypair::load_or_generate(default) + 同 im.db
    //
    //    home 长跑机器必须用持久化版本，否则 device token 重启失效（用户每次还得
    //    重 /pair）+ VAPID 公钥变 → 已订阅 push 全废。
    let im_db_path = im_db::default_db_path()
        .ok_or_else(|| anyhow::anyhow!("无法解析 im.db 路径：$HOME 未设置"))?;
    let im_pool = im_db::init_at(&im_db_path)
        .await
        .with_context(|| format!("打开/迁移 im.db: {}", im_db_path.display()))?;

    let hmac_secret = HmacSecret::load_or_create_default()
        .map_err(|e| anyhow::anyhow!("HMAC 密钥加载失败: {e}"))?;
    let devices = DeviceStore::new(im_pool.clone());
    let im_auth = ImAuth::with_persistence(hmac_secret, devices);

    let vapid_path = fuxi_im::push::keypair::default_keypair_path()
        .ok_or_else(|| anyhow::anyhow!("无法解析 VAPID 文件路径：$HOME 未设置"))?;
    let vapid = fuxi_im::push::keypair::load_or_generate(&vapid_path)
        .map_err(|e| anyhow::anyhow!("VAPID 密钥加载/生成失败: {e}"))?;
    // hooks 的 HyperPushSender 需要 keypair 独立 Arc——同一份 VAPID 数据装两个
    // Arc：state 用于 / vapid-pub 暴露公钥，sender 用于真签名推送。
    let vapid_for_hooks = Arc::new(vapid.clone());
    let im_push = ImPush::with_persistence(vapid, im_pool.clone());

    let app_state = AppState::new(fuxi.clone())
        .with_im_auth(im_auth)
        .with_im_push(im_push);

    // 5. push hooks —— 订阅 EventBus 触发 web push（玄女 idle / task done）。
    //    必须在 fuxi 已 ready 之后挂；玄女 id 若此刻为 None 就 fallback 到内存
    //    UUID（push 钩子里 UserPrompted 不会被自然触发，但 task done 仍能推）。
    //    生产首启场景下玄女经 REPL spawn 即就位；home daemon 部署期还没 REPL，
    //    所以 xuannv_id 大概率为 None——下面 spawn_when_ready 在拿到 id 前阻塞订阅。
    let hooks_pool = im_pool.clone();
    let hooks_bus = bus.clone();
    let hooks_fuxi = fuxi.clone();
    let push_hooks_task = tokio::spawn(async move {
        let xuannv = wait_for_xuannv(&hooks_fuxi).await;
        let sender = Arc::new(HyperPushSender::new(vapid_for_hooks));
        let _h = fuxi_im::push::hooks::spawn(hooks_pool, sender, hooks_bus, xuannv);
        // hooks::spawn 返回的 JoinHandle 持续到订阅流结束，由它自己清理；这里
        // detach 不动它即可（task 进程退出统一终结）。
    });

    // 6. Daemon socket + 策府（与 up.rs 同步逻辑；home 也要 daemon 让 TUI /pair 等
    //    工具可用）
    let oracle = OracleStore::connect_file(&events_db_path)
        .await
        .with_context(|| format!("打开策府 SQLite {}", events_db_path.display()))?;
    let sock_path = args.sock_path.clone().unwrap_or_else(ipc::socket_path);
    fuxi.set_recall_sink(Arc::new(crate::recall_sink::OracleRecallSink::new(
        oracle.clone(),
    )))
    .await;

    // 6.5 自启玄女（Task #8）。home 长跑场景下用户不必先 ssh 跑 REPL——`fuxi im start`
    //     直接把玄女拉起来，PWA 第一次 `/api/conv` 就有人对面。
    //
    //     幂等：xuannv_bootstrap::ensure_xuannv 见到 xuannv_id 已 Some 直接返回；
    //     重启场景下 set_xuannv 在进程内是丢的（内存态），但 cc 的 session 由策府
    //     resolve_xuannv_session 保留，cc --resume 拉回上次上下文。
    //
    //     失败语义：role 加载错或 cc launch 失败 → fail-fast 让 systemd 重启；
    //     这是部署期就该看到的错（roles 路径错配 / cc 没装），不该静默降级。
    let xuannv_role = std::env::var("FUXI_IM_XUANNV_ROLE")
        .unwrap_or_else(|_| crate::xuannv_bootstrap::DEFAULT_XUANNV_ROLE.to_string());
    match crate::xuannv_bootstrap::ensure_xuannv(&fuxi, &oracle, &xuannv_role).await {
        Ok(id) => {
            // set_xuannv 已经在 ensure_xuannv 里调用——上面 step 5 的 push_hooks_task
            // 内部 wait_for_xuannv 会在下一次 2s 轮询拿到这个 id（旧设计的轮询路径
            // 不动，避免 step 5 / 6.5 顺序耦合）。
            tracing::info!(xuannv = %id, role = %xuannv_role, "玄女自启完成");
        }
        Err(e) => {
            // 即便自启失败也别让整个 daemon 死——降级到"等 REPL 起玄女"路径，
            // 让 push hooks 的 wait_for_xuannv 继续轮询兜底，IM API 仍可用。
            tracing::warn!(error = %e, role = %xuannv_role,
                "玄女自启失败，降级等 REPL 启动；PWA /api/conv 在玄女就位前会 503");
        }
    }
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

    // 7. webhook router（scheduler）+ IM router + ServeDir(/) 合并
    let webhook_router = fuxi_scheduler::webhook::router(WebhookState {
        store: sched_store.clone(),
        keeper: keeper.clone(),
    });
    let im_router = fuxi_im::router(app_state);
    let web_root = args.web_root.clone().or_else(default_web_root);
    let app = match web_root {
        Some(root) if root.is_dir() => {
            tracing::info!(web_root = %root.display(), "PWA dist 已挂载到 /");
            im_router
                .merge(webhook_router)
                .fallback_service(ServeDir::new(root))
        }
        Some(root) => {
            tracing::warn!(
                web_root = %root.display(),
                "PWA dist 不存在——/ 将返 404；install.sh 没把 dist 推到位？"
            );
            im_router.merge(webhook_router)
        }
        None => {
            tracing::warn!("未指定 PWA dist 路径（--web-root + 默认都失败）；/ 将无静态资源");
            im_router.merge(webhook_router)
        }
    };

    // 8. PlatformStarted
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    })
    .ok();
    tracing::info!(
        addr = %args.bind,
        sock = %sock_path.display(),
        events_db = %events_db_path.display(),
        im_db = %im_db_path.display(),
        "fuxi im start 上线"
    );
    eprintln!(
        "fuxi im start\n  HTTP  http://{}  (PWA / · API /api · WS /api/conv /api/tasks/:id/stream)\n  SOCK  {} (玄女工具口)\n  DB    events={} · im={}\n  Ctrl-C 停止",
        args.bind,
        sock_path.display(),
        events_db_path.display(),
        im_db_path.display()
    );

    // 9. axum serve + graceful shutdown
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

    daemon_shutdown.notify_waiters();
    let _ = daemon_task.await;
    keeper_task.abort();
    fs_rig.join.abort();
    drop(fs_rig);
    push_hooks_task.abort();

    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStopping,
    })
    .ok();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    result.context("axum serve 异常")
}

/// 等玄女 spawn 出来——当 push hooks 想准确判断"是不是玄女回响应"时需要她的
/// AgentId。home 长跑期间玄女早晚由 REPL 起来。在此之前，hooks task 会被 await
/// 阻塞但 IM API 已可用。
///
/// 兜底：如果 5 分钟内还没玄女（没有人开 REPL），fallback 到内存 UUID——push
/// hooks 仍能跑 task done 路径（基于 task_id），玄女 idle 路径会因不匹配 agent
/// 而静默——可接受降级。
async fn wait_for_xuannv(fuxi: &Fuxi) -> fuxi_core::id::AgentId {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if let Some(id) = fuxi.xuannv_id().await {
            return id;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("玄女 5 分钟未上线，push hooks 用 placeholder agent id 兜底");
            return fuxi_core::id::AgentId::new();
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

fn default_events_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("events.db"))
}

fn default_workspace_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("fuxi-workspace"))
}

fn default_web_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join(".local")
            .join("share")
            .join("fuxi")
            .join("im-web")
    })
}

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
