//! handler 间共享的应用状态。
//!
//! 字段约定：**`AppState::new(fuxi)` 必须永远是最小可构造路径**——骨架 smoke
//! 测试 + 各 owner 单元测试都靠它装配 router；新加字段都给"默认即可用"的占位
//! （内存 HMAC + 空 PendingPairs 等），让没接 production wiring 的测试照旧 200。
//! production daemon 启动期通过 `with_*` builder 注入真路径（文件密钥 / im.db）。
//!
//! 为什么用 `Arc<Fuxi>` 而不是 owned：handler 是 `'static` 任务，必须 cheap clone。

use crate::auth::HmacSecret;
use crate::conv_store::ConvStore;
use crate::devices::DeviceStore;
use crate::handlers::vision::{FrameError, FrameRecord};
use crate::lockout::LoginGuard;
use crate::nodes_provider::NodesProvider;
use crate::notifications::NotificationStore;
use crate::pair::PendingPairs;
use crate::push::VapidKeypair;
use crate::uploads::UploadStore;
use fuxi_memory::{HetuStore, OracleStore, UserProfileStore};
use fuxi_orchestrator::Fuxi;
use fuxi_scheduler::TriggerStore;
use fuxi_workspace::FileSystemProjectRegistry;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

/// 玄女眼睛 v1 oneshot 配对表别名——`request_id → sender`。
/// `look` handler 注入 sender，`look/frame` handler 上传时 take 出来 send。
/// 走 `tokio::sync::Mutex` 而不是 `std::sync::Mutex`：handler 跨 await 持锁时
/// std mutex 不安全（虽然这里不该跨 await，但 tokio mutex 是 axum 共享 state
/// 的常规选择，clippy 也鼓励）。
pub type VisionPairs =
    Arc<Mutex<HashMap<String, oneshot::Sender<std::result::Result<FrameRecord, FrameError>>>>>;

/// 共享给所有 handler 的应用状态。`Clone` 廉价（内部都是 `Arc`）。
#[derive(Clone)]
pub struct AppState {
    /// 玄女编排句柄——`/api/intervene` / `/api/dispatch` / `/api/tasks` 全要它。
    pub fuxi: Arc<Fuxi>,
    /// β · 设备配对 + token 签发 + 持久化 device store。
    pub im_auth: ImAuth,
    /// δ · Web Push 域：VAPID keypair + im.db 句柄。
    pub im_push: ImPush,
    /// β · #17 IM 层聊天记录（conversations + messages）持久层。
    /// `Option`：测试 / smoke 默认 None；handler 看到 None 应返 503。
    pub conv_store: Option<ConvStore>,
    /// β · #17 文件上传持久层（uploads 表 + 落盘）。
    /// `Option`：同上，None 时上传/下载 handler 返 503。
    pub upload_store: Option<UploadStore>,
    /// β · #55 节点拓扑提供方——production 由 fuxi-cli 注入 `Arc<DistController>`
    /// 包装。`Option`：smoke / 单测默认 None；handler 返 503。
    /// 见 `crate::nodes_provider::NodesProvider` trait + `handlers::nodes`。
    pub nodes_provider: Option<Arc<dyn NodesProvider>>,
    /// β · #56 dist worker onboarding 派发的 secret/token 三件套。
    /// production 由 fuxi-cli 在 `im_dist::build_dist_layer` 后注入；
    /// `Option`：smoke / 单测默认 None；setup-worker handler 返 503。
    pub dist_secrets: Option<DistSecrets>,
    /// Decision 21 phase 1：Project 注册表——`/api/projects` 数据源。
    /// `Option`：smoke / 单测默认 None；handler 返 503。production
    /// `fuxi im start` 用 `FileSystemProjectRegistry::with_default_root()` 注入。
    pub project_registry: Option<Arc<FileSystemProjectRegistry>>,
    /// v1-session16 通知 tab：bug 收集器 + 系统通知 + 后续 context handoff offer
    /// 都进同一张 notifications 表。production 注入 NotificationStore；smoke /
    /// 测试默认 None，handler 返 503。
    pub notifications: Option<NotificationStore>,
    /// v1-session17 task #9 「更多 → 记忆」：玄女策府 oracle 句柄。
    /// production 走 `~/.fuxi/events.db` 同库（同 fuxi-cli `im.rs` 的 `OracleStore::connect_file`）；
    /// `Option`：smoke / 单测默认 None；handler 返 503。
    pub oracle: Option<OracleStore>,
    /// v1-session19：河图洛书 / 仓颉 insight 持久层。
    /// `/api/memory` 拉它做 PWA「记忆」页 insight section 数据源——之前只读 oracle 一层
    /// 让玄女自报现实只显 session_id/worktree 噪音，hetu 真知识被埋没。
    /// `Option`：smoke / 单测默认 None；handler 视作 0 条。
    pub hetu: Option<HetuStore>,
    /// v1-session19：用户身份卡（`fuxi profile set` / `cangjie` 抽出来的标签）。
    /// 同上接到 PWA 记忆页第三 section。
    pub user_profile_store: Option<UserProfileStore>,
    /// v1-session17 task #9 「更多 → 更漏」：scheduler trigger 注册表。
    /// production 共用 events.db 上的 triggers 表；`Option` None 时 handler 返 503。
    pub triggers: Option<TriggerStore>,
    /// v1-session17 task #9 「更多 → 角色」：roles 目录根（含 `<role>/ROLE.md`）。
    /// production = 项目根 `roles/`；`Option` None 时 handler 返 503。
    pub roles_root: Option<PathBuf>,
    /// 玄女眼睛 v1：`/api/xuannv/look` ↔ `/api/xuannv/look/frame` 的 oneshot
    /// 配对表。每次 `look` 调用注入 sender，桌宠 frame 上传时按 request_id 反
    /// 查 take 出来通知阻塞中的 caller。**默认空 Arc——production 与 smoke
    /// 路径都共用同一个空 map，无需 wiring**（与 conv_store/oracle 等可选字段
    /// 不同，vision 是无外部依赖的内存映射）。
    pub vision_pairs: VisionPairs,
}

/// β · #56 dist worker onboarding 派给本地 macOS 节点的三件套。
/// 通过 `POST /api/dist/setup-worker` 主密码鉴权后下发——
/// install-local-worker.sh 写到 `~/.fuxi/dist-worker.env`。
///
/// **不暴露 setter**——这些值由 `fuxi im start` 启动时一次性加载/生成
/// （见 `fuxi-cli/src/im_dist.rs`），运行期不变更。
#[derive(Clone, Debug)]
pub struct DistSecrets {
    /// HMAC secret 明文——同 fuxi-im 启动时 `FUXI_DIST_HMAC_SECRET` env / 落盘文件
    /// 加载/生成的那个值。脚本写到 worker 端 `FUXI_DIST_HMAC_SECRET`。
    pub hmac_secret: String,
    /// dist token 明文——脚本写到 worker 端 `FUXI_DIST_TOKEN`。
    pub dist_token: String,
    /// dist controller URL——一般 `<home_url>/dist`，部署侧 nginx 反代到 fuxi-im :9100。
    pub controller_url: String,
}

/// β 鉴权相关的子 state——子结构方便整体 clone 同时 handler 用 `state.im_auth.*`
/// 一目了然属于哪个域。
#[derive(Clone)]
pub struct ImAuth {
    /// 全局 HMAC 密钥——sign / verify token 用。
    pub secret: Arc<HmacSecret>,
    /// 内存 PIN 表——TUI `/pair` 投入、`/api/auth/pair` 消费（fallback 路径）。
    pub pairs: Arc<PendingPairs>,
    /// `device_tokens` 持久层——配对/登入成功入库；`/devices` 读 / revoke。
    /// `Option`：测试场景下没 db pool 可注入；handler 处理 None 视作"持久化关闭"
    /// （仍签 token，跳过入库）——这样 router_smoke 不必起 sqlx。
    pub devices: Option<DeviceStore>,
    /// β · #9 主密码 hash 文件路径。`Option`：测试默认无；production 用
    /// `password::default_path()`。文件**可能不存在**——handler 应返 503 引导用户
    /// 跑 `fuxi im set-password`。
    pub password_path: Option<Arc<PathBuf>>,
    /// β · #9 IP 维度登入失败计数 + 锁定守卫。
    pub login_guard: Arc<LoginGuard>,
}

impl AppState {
    /// 用一个已经构造好的 `Fuxi` 句柄装配 state。
    ///
    /// `im_auth` / `im_push` 走默认占位：随机内存 HMAC key + 空 PendingPairs +
    /// 无 DeviceStore + 无 VAPID keypair + 无 push db pool。
    /// production 部署调 `with_im_auth(...)` / `with_im_push(...)` 注入真实 wiring。
    pub fn new(fuxi: Arc<Fuxi>) -> Self {
        Self {
            fuxi,
            im_auth: ImAuth::ephemeral(),
            im_push: ImPush::disabled(),
            conv_store: None,
            upload_store: None,
            nodes_provider: None,
            dist_secrets: None,
            project_registry: None,
            notifications: None,
            oracle: None,
            hetu: None,
            user_profile_store: None,
            triggers: None,
            roles_root: None,
            vision_pairs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 注入完整鉴权 wiring——daemon 启动期用。
    pub fn with_im_auth(mut self, im_auth: ImAuth) -> Self {
        self.im_auth = im_auth;
        self
    }

    /// 注入完整 push wiring——daemon 启动期用（生成或加载 VAPID + 共用 im.db pool）。
    pub fn with_im_push(mut self, im_push: ImPush) -> Self {
        self.im_push = im_push;
        self
    }

    /// 注入聊天记录持久层（Task #17）。
    pub fn with_conv_store(mut self, store: ConvStore) -> Self {
        self.conv_store = Some(store);
        self
    }

    /// 注入文件上传持久层（Task #17）。
    pub fn with_upload_store(mut self, store: UploadStore) -> Self {
        self.upload_store = Some(store);
        self
    }

    /// β · #55 注入节点拓扑数据源——`fuxi-cli/src/im_dist.rs::build_dist_layer`
    /// 后由 caller 包 `Arc<DistController>` 调本方法。
    pub fn with_nodes_provider(mut self, provider: Arc<dyn NodesProvider>) -> Self {
        self.nodes_provider = Some(provider);
        self
    }

    /// β · #56 注入 dist secret/token onboarding 包——`fuxi im start` 启动时
    /// 用同一组 secret 配 [`with_nodes_provider`] 一并注入。
    pub fn with_dist_secrets(mut self, secrets: DistSecrets) -> Self {
        self.dist_secrets = Some(secrets);
        self
    }

    /// Decision 21 phase 1：注入 Project 注册表，激活 `/api/projects` 端点。
    pub fn with_project_registry(mut self, registry: FileSystemProjectRegistry) -> Self {
        self.project_registry = Some(Arc::new(registry));
        self
    }

    /// v1-session16：注入通知存储，激活 `/api/notifications`。
    pub fn with_notifications(mut self, store: NotificationStore) -> Self {
        self.notifications = Some(store);
        self
    }

    /// v1-session17 task #9：注入策府 oracle 句柄，激活 `/api/memory` 第一 section。
    pub fn with_oracle(mut self, oracle: OracleStore) -> Self {
        self.oracle = Some(oracle);
        self
    }

    /// v1-session19：注入河图洛书 insight 持久层，激活 `/api/memory` insight section。
    pub fn with_hetu(mut self, hetu: HetuStore) -> Self {
        self.hetu = Some(hetu);
        self
    }

    /// v1-session19：注入用户身份卡持久层，激活 `/api/memory` profile section。
    pub fn with_user_profile_store(mut self, store: UserProfileStore) -> Self {
        self.user_profile_store = Some(store);
        self
    }

    /// v1-session17 task #9：注入更漏 trigger 注册表，激活 `/api/cron`。
    pub fn with_triggers(mut self, triggers: TriggerStore) -> Self {
        self.triggers = Some(triggers);
        self
    }

    /// v1-session17 task #9：声明 roles 目录根（`<role>/ROLE.md`），激活 `/api/roles`。
    pub fn with_roles_root(mut self, root: PathBuf) -> Self {
        self.roles_root = Some(root);
        self
    }
}

/// δ Web Push 子 state——子结构方便整体 clone 同时 handler 用 `state.im_push.*`
/// 一目了然属于哪个域。
///
/// 测试场景下（router_smoke）`im_push = ImPush::disabled()`：handler 看到
/// `keypair=None || db=None` 应返 503/501 而不是 panic。
#[derive(Clone, Default)]
pub struct ImPush {
    /// VAPID keypair——sign 推送 + 暴露公钥给前端 `applicationServerKey`。
    pub keypair: Option<Arc<VapidKeypair>>,
    /// im.db SqlitePool：push_subscriptions 表读写共用 β 的 device_tokens db。
    pub db: Option<SqlitePool>,
}

impl ImPush {
    /// 全空：无 VAPID 也无 db pool——push 域离线。router_smoke 默认走这条。
    pub fn disabled() -> Self {
        Self::default()
    }

    /// production 注入：完整 keypair + db pool。
    pub fn with_persistence(keypair: VapidKeypair, db: SqlitePool) -> Self {
        Self {
            keypair: Some(Arc::new(keypair)),
            db: Some(db),
        }
    }
}

impl ImAuth {
    /// 进程内瞬态：随机生成 32 字节 HMAC key + 空 pairs + 无 db + 无 password。
    /// 测试用——重启后 token 全失效，但 smoke 路径不依赖持久化。
    pub fn ephemeral() -> Self {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        let secret = HmacSecret::from_string(URL_SAFE_NO_PAD.encode(buf));
        Self {
            secret: Arc::new(secret),
            pairs: Arc::new(PendingPairs::new()),
            devices: None,
            password_path: None,
            login_guard: Arc::new(LoginGuard::new()),
        }
    }

    /// production 注入：文件 key + im.db SqlitePool 包装的 DeviceStore。
    /// `password_path` 默认 `~/.fuxi/im_password.bcrypt`（`HOME` 缺时为 None）。
    pub fn with_persistence(secret: HmacSecret, devices: DeviceStore) -> Self {
        Self {
            secret: Arc::new(secret),
            pairs: Arc::new(PendingPairs::new()),
            devices: Some(devices),
            password_path: crate::password::default_path().map(Arc::new),
            login_guard: Arc::new(LoginGuard::new()),
        }
    }

    /// 显式覆盖主密码文件路径（单测注入临时路径）。
    pub fn with_password_path(mut self, path: PathBuf) -> Self {
        self.password_path = Some(Arc::new(path));
        self
    }
}
