//! handler 间共享的应用状态。
//!
//! 字段约定：**`AppState::new(fuxi)` 必须永远是最小可构造路径**——骨架 smoke
//! 测试 + 各 owner 单元测试都靠它装配 router；新加字段都给"默认即可用"的占位
//! （内存 HMAC + 空 PendingPairs 等），让没接 production wiring 的测试照旧 200。
//! production daemon 启动期通过 `with_*` builder 注入真路径（文件密钥 / im.db）。
//!
//! 为什么用 `Arc<Fuxi>` 而不是 owned：handler 是 `'static` 任务，必须 cheap clone。

use crate::auth::HmacSecret;
use crate::devices::DeviceStore;
use crate::pair::PendingPairs;
use crate::push::VapidKeypair;
use fuxi_orchestrator::Fuxi;
use sqlx::SqlitePool;
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
}

/// β 鉴权相关的子 state——子结构方便整体 clone 同时 handler 用 `state.im_auth.*`
/// 一目了然属于哪个域。
#[derive(Clone)]
pub struct ImAuth {
    /// 全局 HMAC 密钥——sign / verify token 用。
    pub secret: Arc<HmacSecret>,
    /// 内存 PIN 表——TUI `/pair` 投入、`/api/auth/pair` 消费。
    pub pairs: Arc<PendingPairs>,
    /// `device_tokens` 持久层——配对成功入库；`/devices` 读 / revoke。
    /// `Option`：测试场景下没 db pool 可注入；handler 处理 None 视作"持久化关闭"
    /// （仍签 token，跳过入库）——这样 router_smoke 不必起 sqlx。
    pub devices: Option<DeviceStore>,
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
    /// 进程内瞬态：随机生成 32 字节 HMAC key + 空 pairs + 无 db。
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
        }
    }

    /// production 注入：文件 key + im.db SqlitePool 包装的 DeviceStore。
    pub fn with_persistence(secret: HmacSecret, devices: DeviceStore) -> Self {
        Self {
            secret: Arc::new(secret),
            pairs: Arc::new(PendingPairs::new()),
            devices: Some(devices),
        }
    }
}
