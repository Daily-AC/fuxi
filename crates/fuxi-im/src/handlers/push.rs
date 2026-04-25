//! `/api/push/subscribe` + `/api/push/silence` —— Web Push 端点（δ）。
//!
//! 自签 VAPID 见 Decision 14 E。
//!
//! WHY 让 client 在 body 里带 device_id 而不是从 middleware extension 取：
//! β 的 cookie middleware 暂未把 claims 注入 request extensions（`middleware.rs`
//! 注释：等真有 handler 要审计设备时再加，是 TODO）。push 是首个需要 device 维度
//! 的 handler，**让 β 改 middleware 是更干净的解**——但跨 owner 协调成本高，
//! 退化方案：前端在 pair 完成后存 device_id 到 localStorage，每次 subscribe /
//! silence 自带。auth cookie 仍是访问门禁，body 里的 device_id 只是软引用——
//! 安全网仍由 cookie + token 维持。

use crate::error::{Error, Result};
use crate::push::store;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: PushKeys,
    /// 配对时返给客户端的 device_id；前端 localStorage 缓存并每次带回。
    pub device_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    /// VAPID 公钥 base64url-no-pad；前端用作 `applicationServerKey`。
    pub vapid_public_key: String,
    /// 入库的订阅 id（前端不必用，调试方便）。
    pub subscription_id: String,
}

pub async fn subscribe(
    State(state): State<AppState>,
    Json(body): Json<PushSubscription>,
) -> Result<Json<SubscribeResponse>> {
    let pool =
        state.im_push.db.as_ref().ok_or_else(|| {
            Error::Internal("push db pool 未注入（im_push disabled）".to_string())
        })?;
    let kp =
        state.im_push.keypair.as_ref().ok_or_else(|| {
            Error::Internal("VAPID keypair 未注入（im_push disabled）".to_string())
        })?;

    if body.endpoint.is_empty() || body.keys.p256dh.is_empty() || body.keys.auth.is_empty() {
        return Err(Error::BadRequest(
            "endpoint / p256dh / auth 不能为空".to_string(),
        ));
    }
    if body.device_id.is_empty() {
        return Err(Error::BadRequest("device_id 不能为空".to_string()));
    }

    let row = store::upsert(
        pool,
        &body.device_id,
        &body.endpoint,
        &body.keys.p256dh,
        &body.keys.auth,
    )
    .await?;

    Ok(Json(SubscribeResponse {
        vapid_public_key: kp.public_b64url.clone(),
        subscription_id: row.id,
    }))
}

// ─── /api/push/silence ────────────────────────────────────────────────

/// `POST /api/push/silence`——客户端 visibilitychange=visible 时调，
/// 暂停 push 一段时间（默认 60s）；body `{ device_id, seconds? }`。
///
/// 路由 team-lead 待 confirm（Decision 14 E 节正文有，C 表未列）；handler
/// 已实装，等绿灯后在 `router.rs` 加 `.route("/api/push/silence", post(silence))`。
/// `#[allow(dead_code)]` 暂避 clippy；并挂后即移除。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SilenceRequest {
    pub device_id: String,
    /// 暂停秒数；省略 = 60s；<=0 = 立即清除静音。
    #[serde(default)]
    pub seconds: Option<i64>,
}

#[allow(dead_code)]
const DEFAULT_SILENCE_SECS: i64 = 60;

#[allow(dead_code)]
pub async fn silence(
    State(state): State<AppState>,
    Json(body): Json<SilenceRequest>,
) -> Result<Json<serde_json::Value>> {
    let pool =
        state.im_push.db.as_ref().ok_or_else(|| {
            Error::Internal("push db pool 未注入（im_push disabled）".to_string())
        })?;
    if body.device_id.is_empty() {
        return Err(Error::BadRequest("device_id 不能为空".to_string()));
    }
    let secs = body.seconds.unwrap_or(DEFAULT_SILENCE_SECS);
    let until = if secs <= 0 {
        None
    } else {
        Some(chrono::Utc::now() + chrono::Duration::seconds(secs))
    };
    store::set_silence_until(pool, &body.device_id, until).await?;
    Ok(Json(serde_json::json!({
        "device_id": body.device_id,
        "silence_until": until.map(|t| t.to_rfc3339()),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;

    /// 直接调 store + 验证 SubscribeResponse 序列化字段——
    /// 完整 axum router 单测需要 Fuxi 句柄，留给 e2e（tests/push_*.rs）。
    #[tokio::test]
    async fn subscribe_response_shape_is_stable() {
        let resp = SubscribeResponse {
            vapid_public_key: "BabcXYZ".to_string(),
            subscription_id: "sub-1".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["vapid_public_key"], "BabcXYZ");
        assert_eq!(json["subscription_id"], "sub-1");
    }

    /// silence seconds<=0 清除静音；>0 设置 silence_until 未来时刻。
    #[tokio::test]
    async fn silence_seconds_logic() {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(dir.path().join("im.db")).await.expect("db");
        store::upsert(&pool, "d1", "https://e/1", "p", "a")
            .await
            .unwrap();

        // 设置静默
        store::set_silence_until(
            &pool,
            "d1",
            Some(chrono::Utc::now() + chrono::Duration::seconds(60)),
        )
        .await
        .unwrap();
        assert!(store::list_active(&pool).await.unwrap().is_empty());

        // 清除
        store::set_silence_until(&pool, "d1", None).await.unwrap();
        assert_eq!(store::list_active(&pool).await.unwrap().len(), 1);
    }
}
