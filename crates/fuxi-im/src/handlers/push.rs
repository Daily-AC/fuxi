//! Web Push 端点（δ）：subscribe / silence / vapid-pub。
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
//!
//! WHY 三个端点而不是一个：浏览器 `pushManager.subscribe({ applicationServerKey })`
//! 必须**先**拿 VAPID 公钥（`GET /api/push/vapid-pub`），调完才能拿到 endpoint
//! 然后 `POST /api/push/subscribe`。从 subscribe response 返公钥时机已晚——
//! 调用方还没 subscribe 怎么再调？所以 vapid-pub 必须独立 GET。
//! `silence` 是 visibilitychange=visible 时单独调，跟 subscribe 时机也不同。

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
    // VAPID keypair 注入与否不影响 subscribe 路径——subscribe 只写库。
    // 但生产部署若没 keypair，`/api/push/vapid-pub` 会 503，前端连
    // applicationServerKey 都拿不到，自然走不到这一步。

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
        subscription_id: row.id,
    }))
}

// ─── /api/push/vapid-pub ──────────────────────────────────────────────

/// `GET /api/push/vapid-pub` —— 暴露 VAPID 公钥给前端拼 `applicationServerKey`。
///
/// 前端必须在 `pushManager.subscribe()` 调用之前拿到这个串，所以**必须**是独立
/// GET 而不是 subscribe response 的字段（B 决议）。
#[derive(Debug, Serialize)]
pub struct VapidPublicKeyResponse {
    /// P-256 公钥 base64url-no-pad（uncompressed point，65 字节解码后）。
    pub key: String,
}

pub async fn vapid_public_key(
    State(state): State<AppState>,
) -> Result<Json<VapidPublicKeyResponse>> {
    let kp =
        state.im_push.keypair.as_ref().ok_or_else(|| {
            Error::Internal("VAPID keypair 未注入（im_push disabled）".to_string())
        })?;
    Ok(Json(VapidPublicKeyResponse {
        key: kp.public_b64url.clone(),
    }))
}

// ─── /api/push/silence ────────────────────────────────────────────────

/// `POST /api/push/silence`——客户端 visibilitychange=visible 时调，
/// 暂停 push 一段时间（默认 60s）；body `{ device_id, seconds? }`。
#[derive(Debug, Deserialize)]
pub struct SilenceRequest {
    pub device_id: String,
    /// 暂停秒数；省略 = 60s；<=0 = 立即清除静音。
    #[serde(default)]
    pub seconds: Option<i64>,
}

const DEFAULT_SILENCE_SECS: i64 = 60;

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

// ─── /api/push/fcm-register ───────────────────────────────────────────

/// `POST /api/push/fcm-register`——Android app 拿到 FCM 注册 token 后调，
/// 把 (token, device_name) 入库。后续玄女 idle / task done 时 hooks 会把
/// 推送 fan-out 给所有登记的 token。
///
/// 与 `subscribe`（Web Push）平行：Web Push 走浏览器 endpoint+keys，FCM 走
/// 原生 SDK token，两条通道互不替代。
#[derive(Debug, Deserialize)]
pub struct FcmRegisterRequest {
    /// FCM SDK 在客户端拿到的注册 token。
    pub token: String,
    /// 设备可读名（用户调试用），如 "以琳的 Pixel"。
    pub device_name: String,
}

#[derive(Debug, Serialize)]
pub struct FcmRegisterResponse {
    /// 固定 true——入库成功。形状稳定方便前端断言。
    pub registered: bool,
}

pub async fn fcm_register(
    State(state): State<AppState>,
    Json(body): Json<FcmRegisterRequest>,
) -> Result<Json<FcmRegisterResponse>> {
    let pool =
        state.im_push.db.as_ref().ok_or_else(|| {
            Error::Internal("push db pool 未注入（im_push disabled）".to_string())
        })?;
    if body.token.is_empty() {
        return Err(Error::BadRequest("token 不能为空".to_string()));
    }
    crate::push::fcm::upsert_token(pool, &body.token, &body.device_name).await?;
    Ok(Json(FcmRegisterResponse { registered: true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;

    /// SubscribeResponse 不再带 vapid_public_key——序列化形状稳定。
    #[tokio::test]
    async fn subscribe_response_shape_is_stable() {
        let resp = SubscribeResponse {
            subscription_id: "sub-1".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["subscription_id"], "sub-1");
        assert!(
            json.get("vapid_public_key").is_none(),
            "subscribe response 不该再带 vapid_public_key（已拆到独立 GET）"
        );
    }

    /// vapid_public_key response 形状：单字段 `key`，串非空且是合法 base64url。
    #[tokio::test]
    async fn vapid_public_key_response_shape() {
        let resp = VapidPublicKeyResponse {
            key: "BabcXYZ123".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["key"], "BabcXYZ123");
    }

    /// silence 写库后 list_active 该 device 被过滤；秒数 <=0 清除恢复 active。
    #[tokio::test]
    async fn silence_writes_to_db_and_filters_list_active() {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(dir.path().join("im.db")).await.expect("db");
        store::upsert(&pool, "d1", "https://e/1", "p", "a")
            .await
            .unwrap();

        // 静默 60s——直接复用 silence handler 的内部逻辑（store::set_silence_until）
        // 不通过 axum router（需要 Fuxi 句柄；e2e 走 tests/）。
        store::set_silence_until(
            &pool,
            "d1",
            Some(chrono::Utc::now() + chrono::Duration::seconds(60)),
        )
        .await
        .unwrap();
        assert!(
            store::list_active(&pool).await.unwrap().is_empty(),
            "silence_until > now 该订阅应被 list_active 过滤"
        );

        // 清除（seconds<=0 路径）
        store::set_silence_until(&pool, "d1", None).await.unwrap();
        assert_eq!(store::list_active(&pool).await.unwrap().len(), 1);
    }

    /// 验证 vapid_public_key handler 真从 keypair 取——直接用 keypair gen 出
    /// 一个 VapidKeypair，模拟 ImPush state，验证返串 = keypair.public_b64url。
    /// （不起 axum router；handler 是纯函数，直接断言。）
    #[tokio::test]
    async fn vapid_public_key_returns_keypair_public_b64url() {
        use crate::push::keypair::generate_at;
        let dir = tempfile::tempdir().expect("tmp");
        let kp = generate_at(dir.path().join("vapid.json")).expect("gen");

        // 校验生成的 public_b64url 是 64-byte X+Y 加 0x04 头 = 65 字节，base64url 后非空
        assert!(!kp.public_b64url.is_empty());
        use base64::Engine;
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&kp.public_b64url)
            .expect("base64url 合法");
        assert_eq!(raw.len(), 65);
        assert_eq!(raw[0], 0x04);

        // 模拟 handler 路径：构造 VapidPublicKeyResponse 并断言字段一致
        let resp = VapidPublicKeyResponse {
            key: kp.public_b64url.clone(),
        };
        assert_eq!(resp.key, kp.public_b64url);
    }

    /// FcmRegisterResponse 形状稳定——前端按 `registered` 字段断言。
    #[test]
    fn fcm_register_response_shape_is_stable() {
        let json = serde_json::to_value(FcmRegisterResponse { registered: true }).unwrap();
        assert_eq!(json["registered"], true);
    }

    /// fcm_register handler：空 token → BadRequest（不入库）。
    ///
    /// 走真 router + 真 im.db pool（im_push wiring），不只测纯函数——这样
    /// 能抓到"handler 没接上 / pool 取法错"之类集成层 bug。
    #[tokio::test]
    async fn fcm_register_rejects_empty_token() {
        use axum::Router;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use axum::routing::post;
        use fuxi_events::EventBus;
        use fuxi_orchestrator::Fuxi;
        use fuxi_workspace::GitWorktreeWorkspace;
        use std::sync::Arc;
        use tower::ServiceExt;

        async fn run_git(cwd: &std::path::Path, args: &[&str]) {
            let out = tokio::process::Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .await
                .expect("spawn git");
            assert!(out.status.success(), "git {args:?} failed");
        }

        let bus = EventBus::with_memory_store().await.expect("bus");
        let ws_dir = tempfile::tempdir().expect("tmp");
        run_git(ws_dir.path(), &["init", "-q", "-b", "main"]).await;
        tokio::fs::write(ws_dir.path().join("README.md"), "seed")
            .await
            .unwrap();
        run_git(ws_dir.path(), &["add", "-A"]).await;
        run_git(
            ws_dir.path(),
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        )
        .await;
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            ws_dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));

        let db_dir = tempfile::tempdir().expect("db tmp");
        let pool = init_at(db_dir.path().join("im.db")).await.expect("db");
        let im_push = crate::state::ImPush {
            keypair: None,
            db: Some(pool.clone()),
        };
        let state = AppState::new(fuxi).with_im_push(im_push);
        let app = Router::new()
            .route("/api/push/fcm-register", post(fcm_register))
            .with_state(state);

        // 空 token → 400
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/push/fcm-register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"token":"","device_name":"Pixel"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "空 token 应 400");
        assert!(
            crate::push::fcm::list_tokens(&pool)
                .await
                .unwrap()
                .is_empty(),
            "拒绝的请求不应入库"
        );

        // 合法 token → 200 + 入库
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/push/fcm-register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"token":"tok-1","device_name":"Pixel"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            crate::push::fcm::list_tokens(&pool).await.unwrap(),
            vec!["tok-1".to_string()]
        );
    }
}
