//! 把通知 fan-out 给所有 active 订阅。
//!
//! 触发方在 [`super::hooks`]——本模块只关心"给 N 个订阅各发一次"。
//!
//! WHY 抽 [`PushSender`] trait：单测里用 `MockSender` 收信纸（mock subscriber
//! endpoint 之外的另一条快路），生产用 [`HyperPushSender`] 走真 web-push。
//! 这种分层和 fuxi-orchestrator/bridge.rs 里的 `Intervener` trait 同款。

use crate::error::{Error, Result};
use crate::push::keypair::VapidKeypair;
use crate::push::store::{self, PushSubscriptionRow};
use async_trait::async_trait;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;

/// 一条 PWA 推送的语义内容；JSON 序列化后作为 Web Push payload。前端
/// service worker 拿到 push event 后解 JSON 渲染 Notification。
#[derive(Debug, Clone, Serialize)]
pub struct PushPayload {
    pub title: String,
    pub body: String,
    /// 点击通知后跳转的相对路径（如 `/#/task/abc`）；前端拼到 origin。
    pub url: String,
}

impl PushPayload {
    pub fn new(title: impl Into<String>, body: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            url: url.into(),
        }
    }
}

/// 单条推送的发送原语；实装由 [`HyperPushSender`] 提供。
///
/// 用 `#[async_trait]` 让 trait 对象可用——`Arc<dyn PushSender>` 是 hooks
/// 注入实装的标准形式。原生 async fn in trait 还不支持 dyn dispatch。
#[async_trait]
pub trait PushSender: Send + Sync {
    /// 推一条；endpoint 失败（4xx/5xx）由实装内部决定是否吞掉只 log——
    /// 单条失败不应阻断 fan-out 其它订阅。
    async fn send_one(&self, sub: &PushSubscriptionRow, payload_json: &[u8]) -> Result<()>;
}

/// VAPID `sub` claim 默认值。RFC 8292 要求 mailto: 或 https: URL；这里用
/// 一个 placeholder mailto——伏羲是个人服务，浏览器只认 sub 存在不验它真。
pub const DEFAULT_VAPID_SUB: &str = "mailto:fuxi@local";

/// `notify` 通用入口：拉 active 订阅、序列化 payload、并行 fan-out。
///
/// 失败一条 = 一条 error log；不打断其余。返 Ok 包"成功送达多少 / 总尝试多少"。
pub async fn notify(
    pool: &SqlitePool,
    sender: Arc<dyn PushSender>,
    payload: &PushPayload,
) -> Result<NotifyOutcome> {
    let active = store::list_active(pool).await?;
    if active.is_empty() {
        return Ok(NotifyOutcome {
            attempted: 0,
            sent: 0,
        });
    }
    let body = serde_json::to_vec(payload)?;
    let mut sent = 0usize;
    for sub in &active {
        match sender.send_one(sub, &body).await {
            Ok(()) => sent += 1,
            Err(e) => tracing::warn!(endpoint = %sub.endpoint, error = %e, "push fan-out 单条失败"),
        }
    }
    Ok(NotifyOutcome {
        attempted: active.len(),
        sent,
    })
}

/// fan-out 结果——给调用方/测试断言用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyOutcome {
    pub attempted: usize,
    pub sent: usize,
}

// ─── HyperPushSender ──────────────────────────────────────────────────

/// 用 `web-push` crate + hyper 后端发送 VAPID 签名的推送。
///
/// 共用一个 [`HyperWebPushClient`]——内部 hyper Client 持 connection pool，
/// fan-out 多条订阅时复用 keep-alive。`VapidKeypair` Arc 共享降到一处加载。
pub struct HyperPushSender {
    keypair: Arc<VapidKeypair>,
    client: web_push::HyperWebPushClient,
    sub: String,
}

impl HyperPushSender {
    pub fn new(keypair: Arc<VapidKeypair>) -> Self {
        Self::with_sub(keypair, DEFAULT_VAPID_SUB)
    }

    pub fn with_sub(keypair: Arc<VapidKeypair>, sub: impl Into<String>) -> Self {
        Self {
            keypair,
            client: web_push::HyperWebPushClient::new(),
            sub: sub.into(),
        }
    }
}

#[async_trait]
impl PushSender for HyperPushSender {
    async fn send_one(&self, sub: &PushSubscriptionRow, payload_json: &[u8]) -> Result<()> {
        use web_push::{
            ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushClient,
            WebPushMessageBuilder,
        };
        let info = SubscriptionInfo::new(&sub.endpoint, &sub.p256dh, &sub.auth);
        // VAPID 签名 builder 需私钥 PEM bytes + 订阅 audience。
        let mut sig = VapidSignatureBuilder::from_pem(self.keypair.private_pem.as_bytes(), &info)
            .map_err(|e| Error::Internal(format!("vapid pem: {e}")))?;
        sig.add_claim("sub", self.sub.clone());
        let signature = sig
            .build()
            .map_err(|e| Error::Internal(format!("vapid sign: {e}")))?;

        let mut builder = WebPushMessageBuilder::new(&info);
        builder.set_payload(ContentEncoding::Aes128Gcm, payload_json);
        builder.set_vapid_signature(signature);
        builder.set_ttl(60 * 60); // 一小时；过 ttl 还没投递 push service 会丢
        let msg = builder
            .build()
            .map_err(|e| Error::Internal(format!("build webpush msg: {e}")))?;
        self.client
            .send(msg)
            .await
            .map_err(|e| Error::Internal(format!("send webpush: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use crate::push::store::upsert;
    use std::sync::Mutex;
    use tempfile::tempdir;

    #[derive(Default)]
    struct MockSender {
        calls: Mutex<Vec<(String, Vec<u8>)>>,
        fail_endpoints: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PushSender for MockSender {
        async fn send_one(&self, sub: &PushSubscriptionRow, payload_json: &[u8]) -> Result<()> {
            let fails = self.fail_endpoints.lock().unwrap().clone();
            self.calls
                .lock()
                .unwrap()
                .push((sub.endpoint.clone(), payload_json.to_vec()));
            if fails.iter().any(|e| e == &sub.endpoint) {
                return Err(Error::Internal("mock fail".into()));
            }
            Ok(())
        }
    }

    async fn fresh_pool() -> SqlitePool {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        std::mem::forget(dir);
        pool
    }

    /// 0 active 订阅时 notify Ok 但 sent=0——空状态不报错。
    #[tokio::test]
    async fn notify_with_no_subs_is_ok() {
        let pool = fresh_pool().await;
        let mock = Arc::new(MockSender::default());
        let payload = PushPayload::new("玄女在等你", "", "/#/conv");
        let out = notify(&pool, mock.clone(), &payload).await.expect("notify");
        assert_eq!(
            out,
            NotifyOutcome {
                attempted: 0,
                sent: 0
            }
        );
        assert!(mock.calls.lock().unwrap().is_empty());
    }

    /// 多条订阅 → 各推一次；JSON payload 内含 title/body/url。
    #[tokio::test]
    async fn notify_fan_out_to_all_active() {
        let pool = fresh_pool().await;
        upsert(&pool, "d1", "https://e/1", "k", "a").await.unwrap();
        upsert(&pool, "d2", "https://e/2", "k", "a").await.unwrap();

        let mock = Arc::new(MockSender::default());
        let payload = PushPayload::new("任务完成", "ERP-1066", "/#/task/abc");
        let out = notify(&pool, mock.clone(), &payload).await.expect("notify");
        assert_eq!(
            out,
            NotifyOutcome {
                attempted: 2,
                sent: 2
            }
        );

        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let endpoints: Vec<_> = calls.iter().map(|(e, _)| e.clone()).collect();
        assert!(endpoints.contains(&"https://e/1".to_string()));
        assert!(endpoints.contains(&"https://e/2".to_string()));

        let body0: serde_json::Value = serde_json::from_slice(&calls[0].1).unwrap();
        assert_eq!(body0["title"], "任务完成");
        assert_eq!(body0["body"], "ERP-1066");
        assert_eq!(body0["url"], "/#/task/abc");
    }

    /// 单条失败不阻断其余——sent < attempted，warn log 出来。
    #[tokio::test]
    async fn notify_continues_on_single_failure() {
        let pool = fresh_pool().await;
        upsert(&pool, "d1", "https://e/1", "k", "a").await.unwrap();
        upsert(&pool, "d2", "https://e/2", "k", "a").await.unwrap();

        let mock = Arc::new(MockSender::default());
        mock.fail_endpoints
            .lock()
            .unwrap()
            .push("https://e/1".into());

        let payload = PushPayload::new("title", "body", "/");
        let out = notify(&pool, mock.clone(), &payload).await.expect("notify");
        assert_eq!(
            out,
            NotifyOutcome {
                attempted: 2,
                sent: 1
            }
        );
    }

    /// silence_until 中的订阅不被 notify 触达。
    #[tokio::test]
    async fn silenced_sub_is_skipped() {
        let pool = fresh_pool().await;
        upsert(&pool, "d1", "https://e/1", "k", "a").await.unwrap();
        upsert(&pool, "d2", "https://e/2", "k", "a").await.unwrap();
        store::set_silence_until(
            &pool,
            "d1",
            Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        )
        .await
        .unwrap();

        let mock = Arc::new(MockSender::default());
        let payload = PushPayload::new("t", "b", "/");
        let out = notify(&pool, mock.clone(), &payload).await.expect("notify");
        assert_eq!(
            out,
            NotifyOutcome {
                attempted: 1,
                sent: 1
            }
        );
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls[0].0, "https://e/2");
    }
}
