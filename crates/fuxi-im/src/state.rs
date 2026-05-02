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
use crate::lockout::LoginGuard;
use crate::nodes_provider::NodesProvider;
use crate::pair::PendingPairs;
use crate::push::VapidKeypair;
use crate::uploads::UploadStore;
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::FileSystemProjectRegistry;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;

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
