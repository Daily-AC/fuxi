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
//! - 内嵌 dist controller（`/dist/*` HMAC；`/api/*` cookie，两套 auth 隔离）
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
use fuxi_core::trigger_lookup::TriggerLookup;
use fuxi_events::{EventBus, EventStore};
use fuxi_im::auth::HmacSecret;
use fuxi_im::db as im_db;
use fuxi_im::devices::DeviceStore;
use fuxi_im::push::fcm::{FcmCredentials, FcmPusher, HttpFcmSender, NoopFcmSender};
use fuxi_im::push::notify::HyperPushSender;
use fuxi_im::state::{AppState, ImAuth, ImPush};
use fuxi_memory::OracleStore;
use fuxi_orchestrator::{
    DEFAULT_TICK_INTERVAL_SECS, Fuxi, FuxiConfig, IdleGcTask, IdleShutdowner,
    InsightExtractorConfig, InsightExtractorTask, SystemEventBridge,
    insight_extractor_config_from_env, ttl_from_env,
};
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
    /// 是否给门客分配 worktree。家用部署默认**关**——home 上没 git repo 当 workspace；
    /// fuxi up（开发用）那边默认 true 不变。`--allocate-worktree=true` 可重开。
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
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

    // v2-session13 收尾：启动期扫 events.db，对"非终态 + 长期无活跃"的 task
    // 兜底发 TaskCancelled——补 Bug 3（commit 8a2e03e）pump 兜底只对新 task
    // 生效的盲点。详见 fuxi_im::orphan_sweep。FUXI_DISABLE_ORPHAN_SWEEP=1 关。
    let _ = fuxi_im::orphan_sweep::sweep_orphan_tasks(&bus).await;

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

    // bug #77：im start 之前**没启** IdleGcTask（只 repl.rs 有），导致 home 上
    // 用户感知"过了好久 worker 还显示待命"。每 30s 扫一次 shelf，超 TTL idle
    // 门客自动回收 → AgentDead 事件 → 前端 status 走 dead 文案。
    let gc_shutdowner: Arc<dyn IdleShutdowner> = fuxi.clone();
    let _gc_task = IdleGcTask::new(
        fuxi.clone_shelf(),
        gc_shutdowner,
        bus.clone(),
        ttl_from_env(),
        std::time::Duration::from_secs(DEFAULT_TICK_INTERVAL_SECS),
    )
    // 块5 步7.2：切到分身池模式——idle_gc 对任一活分身走 dormant 回收（pool.remove +
    // shutdown_idle），分身后续可由 ensure_xuannv_for_topic respawn（7.1）+ general 镜像
    // reconciler 兜（7.4）。**必须在 7.1/7.4 之后切**：否则 general 分身被回收成黑洞。
    .with_xuannv_pool(fuxi.xuannv_pool())
    .spawn();
    tracing::info!(
        ttl_secs = ttl_from_env().as_secs(),
        tick_secs = DEFAULT_TICK_INTERVAL_SECS,
        "IdleGcTask 已启动（分身池 dormant 回收模式——bug #77 修：im start 此前漏启，worker 永不回收）"
    );

    // memory-v2 · 仓颉 InsightExtractorTask（论文 arXiv:2604.14004 Memory Transfer
    // Learning）。subscribe TaskStateChanged Done → spawn 仓颉跑 extraction →
    // judge per insight → 过 0.6 阈值 record 进 hetu_patterns。
    //
    // FUXI_INSIGHT_EXTRACTOR_ENABLED **default true**（v2 跟 v1 反——v1 extractor
    // 是 noise/cost 大默认关，v2 论文支持开）；显式 0/false 关。仓颉 ROLE.md 加载
    // 失败 → warn 不 fail——insight 提取是非关键链路，不能拖死整个 im start。
    match crate::insight_extractor_hook::load_cangjie_launch() {
        Ok((cangjie_profile, cangjie_cc_cfg)) => {
            let hetu = Arc::new(
                fuxi_memory::HetuStore::connect_file(&events_db_path)
                    .await
                    .with_context(|| {
                        format!("打开 hetu_patterns 库 {}", events_db_path.display())
                    })?,
            );
            let spawner = Arc::new(crate::insight_extractor_hook::FuxiCangjieSpawner::new(
                fuxi.clone(),
                bus.clone(),
                cangjie_profile,
                cangjie_cc_cfg,
            ));
            let cfg: InsightExtractorConfig = insight_extractor_config_from_env();
            let enabled = cfg.enabled;
            let _insight_task = InsightExtractorTask::new(bus.clone(), hetu, spawner, cfg).spawn();
            tracing::info!(
                enabled,
                "InsightExtractorTask 已装配（仓颉路径，写入 hetu_patterns insight 层）"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "load_cangjie_launch 失败——InsightExtractorTask 跳过装配，insight 自动提取关闭"
            );
        }
    }

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

    // β · #17 IM 层聊天记录 + 文件上传 wiring
    let conv_store = fuxi_im::conv_store::ConvStore::new(im_pool.clone());
    let upload_root = fuxi_im::uploads::UploadStore::default_root()
        .ok_or_else(|| anyhow::anyhow!("无法解析 ~/.fuxi/im_uploads：$HOME 未设置"))?;
    std::fs::create_dir_all(&upload_root)
        .with_context(|| format!("无法创建上传根目录: {}", upload_root.display()))?;
    let upload_store = fuxi_im::uploads::UploadStore::new(im_pool.clone(), upload_root);

    // β · #54 dist controller 内嵌——必须在 AppState 构造前完成，让 #55
    // NodesProvider 注入闭环（spec gap a + gap c 共要求）。
    // dist_home_dir 复用 events.db 父目录，跟 #54 一致。
    let dist_home_dir = events_db_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let dist_layer = crate::im_dist::build_dist_layer(&dist_home_dir, bus.clone())
        .await
        .context("装配 dist controller layer")?;
    tracing::info!("dist controller 已内嵌——/dist/* 走 HMAC，home 节点已自注册");
    let dist_ctrl = dist_layer.ctrl.clone();
    let dist_router = dist_layer.router;
    // bug #77 home 0/4 修：自心跳从这里起（拿到 fuxi handle 后），inflight 反映
    // 本地非 idle worker 数。每 5s tick。
    crate::im_dist::spawn_home_heartbeat_task(dist_ctrl.clone(), Some(fuxi.clone()));
    let hmac_secret_plain = dist_layer.hmac_secret_plain.clone();
    let dist_token_plain = dist_layer.dist_token_plain.clone();
    // β · #56 dist_secrets 给 /api/dist/setup-worker 派发用。
    // controller_url：用 FUXI_DIST_CONTROLLER_URL env（部署侧 nginx 反代时
    // 指向 https://im.qmledmq.cn:8443/dist），缺则推算 http://<bind>/dist
    // 仅适合 dev——生产部署 ζ 必须设 env。
    let controller_url = std::env::var("FUXI_DIST_CONTROLLER_URL")
        .unwrap_or_else(|_| format!("http://{}/dist", args.bind));

    // P1：home 被自注册为可接活节点后，也必须在 fuxi-im 同进程内起一个
    // 真 worker 消费 pinned_node=home 的 dist job；否则 auto-pin 选 home 时
    // queue 永远无人 pull。用同进程 task，复用 dist worker adapter/事件桥，
    // 不引入额外 systemd 进程。
    crate::dist::spawn_embedded_worker(
        dist_ctrl.clone(),
        crate::dist::DistWorkerArgs {
            controller: controller_url.clone(),
            node: crate::im_dist::HOME_NODE_ID.to_string(),
            token: Some(dist_token_plain.clone()),
            codex_bin: std::env::var("FUXI_HOME_CODEX_BIN").unwrap_or_else(|_| "codex".into()),
            cc_bin: crate::im_dist::resolve_home_cc_bin(),
            poll_ms: std::env::var("FUXI_HOME_WORKER_POLL_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1000),
            tags: crate::im_dist::HOME_NODE_TAGS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_concurrency: crate::im_dist::HOME_NODE_MAX_CONCURRENCY,
            projects_root: None,
        },
        dist_token_plain.clone(),
        Arc::new(crate::dist_auth::HmacSecret::new(hmac_secret_plain.clone())),
    );
    tracing::info!(
        node_id = crate::im_dist::HOME_NODE_ID,
        controller = %controller_url,
        "home embedded dist worker 已启动"
    );
    // β · #55 NodesProvider 包 Arc<DistController>，注入 AppState 让
    // /api/nodes handler 能查 dist topology
    let nodes_provider: Arc<dyn fuxi_im::nodes_provider::NodesProvider> = Arc::new(
        crate::im_dist::DistControllerNodesProvider::new(dist_ctrl.clone()),
    );

    // β · #57 DistEnqueuer 包 Arc<DistController>，注入 Fuxi 让 dispatch
    // 决策树命中 dist 路径时把 task 派到 dist。`set_dist_enqueuer` async setter，
    // 必须 await。
    fuxi.set_dist_enqueuer(Arc::new(crate::im_dist::DistControllerEnqueuer::new(
        dist_ctrl.clone(),
    )))
    .await;
    tracing::info!("Fuxi.dist_enqueuer 已注入——dispatch routing 决策树启用");

    // v2 跨节点 sandbox：NodeLoadProvider 包 Arc<DistController>，注入 Fuxi 让
    // dispatch 在 task 关联到 project 但未显式 pin 时按 inflight/concurrency 选最闲。
    // 这里 clone 而非 move——`dist_ctrl` 后面 `Daemon::new_for_im_start` 还要消费一份。
    fuxi.set_node_load_provider(Arc::new(crate::im_dist::DistNodeLoadProvider::new(
        dist_ctrl.clone(),
    )))
    .await;
    tracing::info!("Fuxi.node_load_provider 已注入——v2 跨节点 sandbox auto-pin 启用");

    let dist_secrets = fuxi_im::state::DistSecrets {
        hmac_secret: hmac_secret_plain,
        dist_token: dist_token_plain,
        controller_url,
    };

    // Decision 21 phase 1：Project 注册表落 $HOME/.fuxi/projects/。
    // $HOME 缺失时跳过注册表注入——/api/projects 会返 503，不致命。
    // 同一 registry 同时给 AppState（PWA 端点）和 Fuxi（spawn_worker_in_project_sandbox）
    // 用——共享一份避免 PWA 看到的 project list 跟 orchestrator 派活时认的脱钩。
    let project_registry = fuxi_workspace::FileSystemProjectRegistry::with_default_root();
    let project_registry_arc = match &project_registry {
        Ok(reg) => Some(Arc::new(reg.clone())),
        Err(_) => None,
    };
    if let Some(reg) = &project_registry_arc {
        fuxi.set_project_registry(reg.clone()).await;
        tracing::info!("Fuxi.project_registry 已注入——spawn_worker_in_project_sandbox 启用");
    }

    // v1-session16 通知 tab：复用 im.db pool，跟 push_subscriptions / device_tokens
    // 同库不同表（migration 0005_notifications.sql 创建）。store 廉价 clone（持
    // SqlitePool Arc 克隆）— AppState 持一份给 PWA 端点，玄女 / orchestrator
    // 后续要写 system 通知时再单独拿一份（task #8）。
    let notification_store = fuxi_im::notifications::NotificationStore::new(im_pool.clone());

    // v1-session17 task #9 「更多」hub 三个新页：
    //   - 策府事实：跟 `fuxi up` 同套路，把 events.db 当 oracle 库（init_schema 幂等）。
    //     section 6 还会再用 oracle.clone() 装 recall_sink，这里提前打开是因为
    //     AppState 装配在 section 4，需要现成 OracleStore 注入。
    //   - 更漏 trigger：上面 step 3 已建 sched_store，clone 一份给 AppState。
    //   - roles 目录：项目根 `roles/` 是 build-time 资产，运行期由 FUXI_ROLES_ROOT
    //     env 覆盖；缺则按 cwd 推 `./roles`，不存在时 with_roles_root 拿到空目录会
    //     扫出空数组（handler 仍 200，前端空态）。
    let oracle = OracleStore::connect_file(&events_db_path)
        .await
        .with_context(|| format!("打开策府 SQLite {}", events_db_path.display()))?;
    // v1-session19 · /api/memory 接 3 层（oracle 之外加 hetu_patterns + user_profile）
    // ——之前只读 oracle 的话用户记忆页全是 session_id 噪音，hetu insight 真知识
    // 显不出来。三库同 events.db，clone 廉价（sqlx Pool 内 Arc）。
    let hetu_for_state = fuxi_memory::HetuStore::connect_file(&events_db_path)
        .await
        .with_context(|| format!("打开河图洛书 SQLite {}", events_db_path.display()))?;
    let user_profile_store = fuxi_memory::UserProfileStore::connect_file(&events_db_path)
        .await
        .with_context(|| format!("打开 user_profile SQLite {}", events_db_path.display()))?;
    let roles_root = std::env::var_os("FUXI_ROLES_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.join("roles"))
                .unwrap_or_else(|_| PathBuf::from("roles"))
        });

    // PWA 语音：wake server 预共享 token 文件。env 覆盖给测试/非常规部署；
    // 缺省 `~/.fuxi/wake.token`（deploy/wake/README.md 约定位置）。文件不存在
    // 不报错——handler 降级 wake_token=null，前端隐藏唤醒开关。
    let wake_token_path = std::env::var_os("FUXI_WAKE_TOKEN_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            fuxi_im::auth::default_fuxi_dir()
                .map(|d| d.join("wake.token"))
                .unwrap_or_else(|_| PathBuf::from("wake.token"))
        });

    // Phase 1 · topic_store 跟 conv_store 同 im.db pool。
    let topic_store = fuxi_im::topic_store::TopicStore::new(im_pool.clone());

    let app_state_base = AppState::new(fuxi.clone())
        .with_im_auth(im_auth)
        .with_im_push(im_push)
        .with_conv_store(conv_store.clone())
        .with_topic_store(topic_store.clone())
        .with_upload_store(upload_store)
        .with_nodes_provider(nodes_provider)
        .with_dist_secrets(dist_secrets)
        .with_notifications(notification_store)
        .with_oracle(oracle.clone())
        .with_hetu(hetu_for_state)
        .with_user_profile_store(user_profile_store)
        .with_triggers(sched_store.clone())
        .with_roles_root(roles_root)
        .with_wake_token_path(wake_token_path);
    let app_state = match project_registry {
        Ok(reg) => app_state_base.with_project_registry(reg),
        Err(e) => {
            tracing::warn!("project_registry 未注入，/api/projects 将返 503: {e}");
            app_state_base
        }
    };

    // 5. push hooks —— 订阅 EventBus 触发 web push（玄女 idle / task done）。
    //    必须在 fuxi 已 ready 之后挂；玄女 id 若此刻为 None 就 fallback 到内存
    //    UUID（push 钩子里 UserPrompted 不会被自然触发，但 task done 仍能推）。
    //    生产首启场景下玄女经 REPL spawn 即就位；home daemon 部署期还没 REPL，
    //    所以 xuannv_id 大概率为 None——下面 spawn_when_ready 在拿到 id 前阻塞订阅。
    let hooks_pool = im_pool.clone();
    let hooks_bus = bus.clone();
    let hooks_fuxi = fuxi.clone();
    // FCM sender：从 ~/.fuxi/fcm_service_account.json 加载 service account。
    // 文件缺失 / 字段不全时**不 panic**——log warn 退化成 NoopFcmSender，
    // 让没配 FCM 的部署（如纯 PWA 场景）照常跑，Web Push 那路不受影响。
    let fcm_sender: Arc<dyn FcmPusher> = match FcmCredentials::load_default() {
        Ok(creds) => {
            tracing::info!("FCM service account 已加载——原生 Android 推送通道启用");
            Arc::new(HttpFcmSender::new(Arc::new(creds)))
        }
        Err(e) => {
            tracing::warn!("FCM service account 未就绪，FCM 推送退化为 no-op: {e}");
            Arc::new(NoopFcmSender)
        }
    };
    let push_hooks_task = tokio::spawn(async move {
        let sender = Arc::new(HyperPushSender::new(vapid_for_hooks));
        // 传分身池 watch 而非 snapshot id：玄女 id 会话内漂移（topic handoff /
        // idle GC 重生），snapshot 烤死后推送静默失效（2026-06-10 实测 bug）。
        // 也因此不必再 wait_for_xuannv——池空时 AgentResponded 自然不匹配，
        // task done 推送则从启动第一秒就可用。
        let _h = fuxi_im::push::hooks::spawn(
            hooks_pool,
            sender,
            fcm_sender,
            hooks_bus,
            hooks_fuxi.xuannv_pool_watch(),
        );
        // hooks::spawn 返回的 JoinHandle 持续到订阅流结束，由它自己清理；这里
        // detach 不动它即可（task 进程退出统一终结）。
    });

    // 6.4 Decision 21 phase 3：L2 ephemeral GC 周期任务——每 1h 扫一次所有
    //     注册项目的 archive/，把 archived_at 早于 (now - 24h) 的归档删除并
    //     publish WorkspaceCollected。registry 缺省时静默跳过（home 部署期可能
    //     还没 ~/.fuxi/projects/）。FUXI_L2_GC_INTERVAL_SECS / FUXI_L2_GC_MAX_AGE_SECS
    //     可覆盖（测试 / 用户调优用）。
    if let Some(reg) = project_registry_arc.clone() {
        let gc_bus = bus.clone();
        let gc_interval_secs = std::env::var("FUXI_L2_GC_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(3_600);
        let gc_max_age_secs = std::env::var("FUXI_L2_GC_MAX_AGE_SECS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(86_400);
        tokio::spawn(async move {
            crate::l2_gc::run(reg, gc_bus, gc_interval_secs, gc_max_age_secs).await;
        });
        tracing::info!(
            interval_secs = gc_interval_secs,
            max_age_secs = gc_max_age_secs,
            "L2 ephemeral GC 周期任务已启动"
        );
    }

    // 6. Daemon socket + 策府（与 up.rs 同步逻辑；home 也要 daemon 让 TUI /pair 等
    //    工具可用）。oracle 在 step 4 已 connect_file 一次（PWA「记忆」用），这里
    //    继续 clone 同一份做 recall_sink——SqlitePool 是 Arc，clone 廉价。
    let sock_path = args.sock_path.clone().unwrap_or_else(ipc::socket_path);
    fuxi.set_recall_sink(Arc::new(crate::recall_sink::OracleRecallSink::new(
        oracle.clone(),
    )))
    .await;

    // 块5：注入持久队列 sink——bridge 在归属 topic 分身 dormant 时把完工/里程碑
    // 信号落 im.db（a01cfab5「信号不丢」），分身 respawn 后 drain 补发（7.4）。
    // 同 im.db pool（SqlitePool Arc，clone 廉价）。
    // 块5：持久队列 store 一份，sink（落库）+ spawner（drain 补发）共用（SqlitePool Arc）。
    let pending_store = fuxi_im::pending_notify::PendingNotifyStore::new(im_pool.clone());
    fuxi.set_pending_sink(Arc::new(crate::pending_sink::PendingNotifyStoreSink::new(
        pending_store.clone(),
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

    // 块5：注入玄女分身懒启动 spawner——ensure_xuannv_for_topic 池 miss / dormant
    // respawn 时调它，复用 spawn_with_prelude + topic 历史拉 prelude。持 Weak<Fuxi>
    // 避免与 Fuxi 持有 spawner 成引用环。必须在 ensure_xuannv 之前注入，让首个 topic
    // 切换/dormant 补发能用上。
    fuxi.set_xuannv_spawner(Arc::new(
        crate::xuannv_spawner_impl::TopicXuannvSpawner::new(
            Arc::downgrade(&fuxi),
            oracle.clone(),
            xuannv_role.clone(),
            conv_store.clone(),
            topic_store.clone(),
            pending_store.clone(),
        ),
    ))
    .await;

    let conv_sync_handle =
        match crate::xuannv_bootstrap::ensure_xuannv(&fuxi, &oracle, &xuannv_role).await {
            Ok(id) => {
                // set_xuannv 已经在 ensure_xuannv 里调用——上面 step 5 的 push_hooks_task
                // 通过 watch::Receiver 立刻收到通知（#7 修：从 2s 轮询切换为 watch
                // changed() 实时唤醒）。
                tracing::info!(xuannv = %id, role = %xuannv_role, "玄女自启完成");
                // β · #17 conv_store sync hook —— 玄女上线后立刻订 EventBus 翻消息进 messages 表。
                // `spawn_xuannv_sync` 是 async 函数，**返回前**完成 ensure_scope + subscribe，
                // 这样玄女后续任何 publish 都不会丢。
                //
                // 传 watch 而不是静态 id：handoff 后 ensure_xuannv 会 spawn 新副本并
                // `set_xuannv(new_id)`，watch::Receiver 立刻看到新值，sync task
                // 用新 id 过滤新副本事件——不切 watch 的话新副本发言全部 drop（
                // bug "handoff 后新玄女副本发言不进 PWA IM 历史"）。
                let h = fuxi_im::conv_store::spawn_xuannv_sync(
                    Arc::new(conv_store.clone()),
                    bus.clone(),
                    fuxi.xuannv_id_watch(),
                    fuxi.current_topic_watch(),
                )
                .await;

                // **CRITICAL** SystemEventBridge 装配——TUI REPL 在 repl.rs:394 装的，
                // IM 重做时漏抄了！没装 bridge 玄女永远收不到 AgentRequestReview /
                // AgentDead 这些系统事件 → 用户实测「门客明明完成了，玄女说没事件」
                // 真因（事件入了 SQLite 但 bridge 没订阅 → 没把 nudge 注入玄女）。
                //
                // trigger_lookup 用 sched_store —— 跟 TUI 同一份 impl（fuxi_scheduler
                // ::TriggerLookup for SchedulerStore）。bridge 任务的 JoinHandle 不存
                // （bridge 是只读订阅，进程结束 tokio runtime drop 自动清）。
                let trigger_lookup: Arc<dyn TriggerLookup> = Arc::new(sched_store.clone());
                SystemEventBridge::spawn(fuxi.clone(), bus.clone(), id, trigger_lookup);
                tracing::info!(xuannv = %id, "SystemEventBridge 已装配");

                // Phase 1 · XuannvSwitcher 反向依赖注入：fuxi-im handler 通过它
                // 切玄女 topic（PWA sidebar `/api/topics/:id/switch`）。
                fuxi.set_xuannv_switcher(Arc::new(
                    crate::topic_switcher_impl::CliXuannvSwitcher::new(
                        fuxi.clone(),
                        oracle.clone(),
                        conv_store.clone(),
                        topic_store.clone(),
                        xuannv_role.clone(),
                    ),
                ))
                .await;
                tracing::info!("XuannvSwitcher 已注入（/api/topics/:id/switch 激活）");

                // task #8 玄女上下文水位监控——35%/45% 跨阈值触发 addendum /
                // handoff offer。订阅 EventBus 上玄女自身的 UsageReport 累加。
                // intervener 走 Fuxi 的 `intervene_system` 注入系统消息。
                let intervener: Arc<dyn fuxi_orchestrator::Intervener> = fuxi.clone();
                let _ctx_watcher = fuxi_orchestrator::xuannv_context::start_watcher(
                    fuxi.xuannv_id_watch(),
                    intervener,
                    bus.clone(),
                );
                tracing::info!("玄女上下文水位监控已装配（35%/45% 阈值）");

                // task #8 handoff 落档检测器——监听 XuannvHandoffWritten 事件
                // → 等当前 turn idle → kill 当前玄女 → 新 spawn（注入 prelude）。
                let _handoff_watcher = crate::xuannv_handoff::start_watcher(
                    fuxi.clone(),
                    bus.clone(),
                    oracle.clone(),
                    xuannv_role.clone(),
                );
                tracing::info!("玄女 handoff 监控已装配");

                h
            }
            Err(e) => {
                // Fail fast：IM 的对话、push idle、deliverable nudge 都依赖玄女上线后
                // 装配 conv_store sync + SystemEventBridge。吞掉这里的错误会把服务留在
                // HTTP online 但核心链路永久 503 的半可用状态。
                return Err(e).with_context(|| format!("玄女自启失败（role={xuannv_role}）"));
            }
        };
    // `new_for_im_start` required-arg 写法编译期防漏 dist_ctrl——commit 之前漏
    // `.with_dist(dist_ctrl)` 让 `fuxi nodes` IPC 报"未启用"误导诊断（玄女 issue
    // #64655b8e/#42184579）。重构这里前先去 `daemon.rs::new_for_im_start` 看注释。
    let daemon = Daemon::new_for_im_start(
        fuxi.clone(),
        bus.clone(),
        sched_store.clone(),
        keeper.clone(),
        oracle,
        dist_ctrl.clone(),
    );
    let daemon_shutdown = daemon.shutdown_handle();
    let sock_for_task = sock_path.clone();
    let daemon_task = tokio::spawn(async move {
        if let Err(e) = daemon.serve(&sock_for_task).await {
            tracing::error!(error = %e, "daemon serve 异常退出");
        }
    });

    // 7. webhook router（scheduler）+ IM router + dist controller router + ServeDir(/) 合并
    //
    // dist_router 已在 step 4.5 提前装配（让 NodesProvider 闭环走通 #55）。
    // /dist/* 走 HMAC layer，/api/* 走 cookie layer，两层互不干扰
    // （cookie layer 的 is_exempt 分支放行 `!path.starts_with("/api/")`）。
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
                .merge(dist_router)
                .fallback_service(ServeDir::new(root))
        }
        Some(root) => {
            tracing::warn!(
                web_root = %root.display(),
                "PWA dist 不存在——/ 将返 404；install.sh 没把 dist 推到位？"
            );
            im_router.merge(webhook_router).merge(dist_router)
        }
        None => {
            tracing::warn!("未指定 PWA dist 路径（--web-root + 默认都失败）；/ 将无静态资源");
            im_router.merge(webhook_router).merge(dist_router)
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
    conv_sync_handle.abort();

    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStopping,
    })
    .ok();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    result.context("axum serve 异常")
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

// ── #9 主密码 set-password 子命令 ────────────────────────────────────

/// `fuxi im set-password` —— 交互式设主密码（β · #9）。
///
/// **硬约束**（team-lead 2026-04-25 拍板）：密码**只**接受 stdin/tty 交互输入。
/// 不提供 `--password <plain>` flag（落 shell history + ps -ef）、不提供
/// `--password-file` flag、不读环境变量 `FUXI_IM_PASSWORD`（leak 风险）。
/// 用户跑这条命令时 rpassword 提示两次输入（不回显），相同 + 校验长度通过后
/// bcrypt cost 12 hash 写 `~/.fuxi/im_password.bcrypt`（权限 0600）。
/// 重跑覆盖旧 hash——即"忘密码重设"语义。
///
/// 测试路径走内部 [`set_password_from_reader`]——接 `BufRead` 喂 `"pwd\npwd\n"`
/// 即可单测，CLI 入口本身是 thin wrapper 不暴露任何"绕过 tty"的口子。
#[derive(Debug, ClapArgs)]
pub struct SetPasswordArgs {
    /// 显式指定密码文件路径，覆盖默认 `~/.fuxi/im_password.bcrypt`。
    /// 部署测试 / 多用户机器（罕见）用得到。本字段**不**接受密码值本身。
    #[arg(long)]
    pub path: Option<PathBuf>,
}

pub async fn run_set_password(args: SetPasswordArgs) -> Result<()> {
    let path = args
        .path
        .clone()
        .or_else(fuxi_im::password::default_path)
        .ok_or_else(|| anyhow::anyhow!("无法解析 ~/.fuxi/im_password.bcrypt：$HOME 未设置"))?;

    let plain = prompt_password_twice_from_tty()?;

    fuxi_im::password::write_password_file(&path, &plain)
        .map_err(|e| anyhow::anyhow!("写入密码文件失败：{e}"))?;

    println!("已写入 {} （权限 0600）", path.display());
    println!("现在可以在 PWA 用这个密码 + 设备名登入。");
    Ok(())
}

/// `fuxi im issue-token` —— 用本机 `~/.fuxi/im_hmac.key` 签一个 HMAC token。
///
/// 给 smoke / 部署后健康检查用：跳过 `/api/auth/login` 流程（不需要主密码 +
/// 设备配对），直接拿到一份可挂 cookie 走 `/api/*` 鉴权的 token。
///
/// 安全：只在能读 `~/.fuxi/im_hmac.key` 的本机用户能签——和 `set-password`
/// 同信任级别（密钥文件权限 0600）。**不要**把签出的 token 写入持久 cookie 文件
/// 或推给其它机器。
///
/// stdout 一行裸 token；用法：
///   `curl -H "Cookie: fuxi_im_token=$(fuxi im issue-token)" https://localhost:9100/api/tasks`
#[derive(Debug, ClapArgs)]
pub struct IssueTokenArgs {
    /// HMAC key 文件路径。默认 `~/.fuxi/im_hmac.key`。
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// token TTL（秒）。默认 3600 = 1 小时；smoke 用足够。
    #[arg(long, default_value_t = 3600)]
    pub ttl_secs: i64,
    /// 写入 claims 的设备名，方便 server 日志辨认。
    #[arg(long, default_value = "smoke-test")]
    pub name: String,
    /// 写入 claims 的 device_id；不给时随机 uuid。verify 不查 device_tokens 表，
    /// 任意值都验得过——给个稳定值便于 grep 日志。
    #[arg(long = "device-id")]
    pub device_id: Option<String>,
}

pub async fn run_issue_token(args: IssueTokenArgs) -> Result<()> {
    use fuxi_im::auth::{TokenClaims, sign_token};

    let secret = match args.key {
        Some(p) => HmacSecret::load_or_create(&p)
            .with_context(|| format!("加载 HMAC key {}", p.display()))?,
        None => HmacSecret::load_or_create_default().context("加载默认 ~/.fuxi/im_hmac.key")?,
    };

    let device_id = args
        .device_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let claims = TokenClaims {
        device_id,
        name: args.name,
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(args.ttl_secs),
    };
    let token = sign_token(&secret, &claims).context("HMAC 签名失败")?;
    println!("{token}");
    Ok(())
}

/// 生产路径：rpassword 直接读 tty 关 echo，不经过任何 fd / pipe，**不可在测试单测**。
/// 单测覆盖 [`set_password_from_reader`]——同样的"读两次 + 比对 + 校验长度"逻辑，
/// 接 BufRead 让 `Cursor::new(b"pwd\npwd\n")` 喂得进去。
fn prompt_password_twice_from_tty() -> Result<String> {
    let first = rpassword::prompt_password("设置主密码（不回显）：")
        .map_err(|e| anyhow::anyhow!("读取密码失败：{e}"))?;
    let second = rpassword::prompt_password("再输一次确认：")
        .map_err(|e| anyhow::anyhow!("读取密码失败：{e}"))?;
    if first != second {
        anyhow::bail!("两次输入不一致");
    }
    Ok(first)
}

/// 测试钩子：从任意 `BufRead` 读两行做"输入 + 确认"——生产**永远不**调到此函数；
/// CLI 入口走 [`prompt_password_twice_from_tty`]。
///
/// 行为契约：
/// - 读两行 trim 末尾换行（`\n` / `\r\n`）做密码
/// - 第二行不等于第一行 → `Err`
/// - 校验长度调 `password::validate_password_strength`
///
/// **不**对外暴露成 pub（除 tests use super::）：避免成为给 attacker 走 fd 注入的口子。
#[cfg(test)]
fn set_password_from_reader<R: std::io::BufRead>(mut reader: R) -> Result<String> {
    let mut first = String::new();
    reader
        .read_line(&mut first)
        .map_err(|e| anyhow::anyhow!("读密码失败：{e}"))?;
    let mut second = String::new();
    reader
        .read_line(&mut second)
        .map_err(|e| anyhow::anyhow!("读密码失败：{e}"))?;
    let first = first.trim_end_matches(['\n', '\r']).to_string();
    let second = second.trim_end_matches(['\n', '\r']).to_string();
    if first != second {
        anyhow::bail!("两次输入不一致");
    }
    fuxi_im::password::validate_password_strength(&first)
        .map_err(|e| anyhow::anyhow!("密码不合规：{e}"))?;
    Ok(first)
}

#[cfg(test)]
mod set_password_tests {
    //! `set_password_from_reader` 内部函数的覆盖——CLI 本身是 thin wrapper：
    //! `run_set_password` = `prompt_password_twice_from_tty` + `password::write_password_file`。
    //! 前者依赖 tty 不可单测（生产路径，由人手验）；后者已在 `fuxi-im` password 模块的
    //! 8 条单测里覆盖（write/read/idempotent/0600/拒短 等）。
    //!
    //! 这里**只**单测 reader 路径——硬约束（team-lead 拍板）：CLI 不接受 flag/env，
    //! 唯一注入口是 stdin/tty，所以 reader 抽象就是覆盖契约的最深底层。

    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_two_matching_lines_returns_password() {
        let input = b"my-pass-good\nmy-pass-good\n";
        let got = set_password_from_reader(Cursor::new(input)).expect("ok");
        assert_eq!(got, "my-pass-good");
    }

    #[test]
    fn read_handles_crlf_line_endings() {
        // Windows / 某些粘贴客户端会带 \r\n；trim_end_matches 应把它们都剥掉
        let input = b"crlf-pass\r\ncrlf-pass\r\n";
        let got = set_password_from_reader(Cursor::new(input)).expect("ok");
        assert_eq!(got, "crlf-pass");
    }

    #[test]
    fn mismatched_confirmation_is_error() {
        let input = b"first-one\nsecond-different\n";
        let err = set_password_from_reader(Cursor::new(input)).expect_err("应拒不一致");
        let msg = format!("{err:#}");
        assert!(msg.contains("不一致"), "错误应明示原因：{msg}");
    }

    #[test]
    fn short_password_is_rejected() {
        let input = b"short\nshort\n";
        let err = set_password_from_reader(Cursor::new(input)).expect_err("应拒短");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("8") || msg.contains("长度"),
            "错误应明示长度限制：{msg}"
        );
    }

    #[test]
    fn empty_password_is_rejected() {
        let input = b"\n\n";
        let err = set_password_from_reader(Cursor::new(input)).expect_err("应拒空");
        let msg = format!("{err:#}");
        // validate_password_strength 给的是"不能为空"或"长度 >= 8"——其一即可
        assert!(
            msg.contains("空") || msg.contains("长度"),
            "错误应明示原因：{msg}"
        );
    }

    /// 验证 reader 路径走完后调用 `password::write_password_file` 的端到端薄壳——
    /// 等价于"如果未来 run_set_password 改成接 reader，行为不退化"的契约锁。
    #[test]
    fn reader_then_write_to_disk_roundtrip() {
        let input = b"valid-pass-12\nvalid-pass-12\n";
        let plain = set_password_from_reader(Cursor::new(input)).expect("read");
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("im_password.bcrypt");
        fuxi_im::password::write_password_file(&path, &plain).expect("write");
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed["version"], 1);
        assert!(parsed["hash"].as_str().unwrap().starts_with("$2b$12$"));
    }

    /// 接口形态契约：SetPasswordArgs 不能含任何"接受密码值"的字段——只允许
    /// 路径 / 元数据 flag。如果将来有人加了 `--password` 之类，此测应 fail。
    #[test]
    fn args_do_not_accept_password_value_flag() {
        // 用反射不现实；改用结构体字段名 textual 检查——本测的存在是给后人
        // 的提醒：**新加字段时，别加任何接受密码值的字段。**
        // SetPasswordArgs 现有字段：path
        let args = SetPasswordArgs { path: None };
        // smoke：能构造即过；编译阶段就会因为字段缺失/多余而失败
        let _ = args;
    }
}
