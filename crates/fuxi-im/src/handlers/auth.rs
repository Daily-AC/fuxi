//! `/api/auth/pair` —— 设备一次性 PIN 配对（β · Decision 14 D）。
//!
//! 流程：TUI `/pair` 调 [`crate::pair::PendingPairs::start`] 出 6 位 PIN →
//! 用户在手机 PWA 输入 PIN + 设备名 POST 此端点 → handler 调 `claim` 验 PIN →
//! 签 token + 写 device_tokens（若注入 DeviceStore）+ Set-Cookie。

use crate::auth::{COOKIE_NAME, build_set_cookie, fresh_claims, sign_token};
use crate::devices::DeviceRecord;
use crate::error::{Error, Result};
use crate::pair::ClaimError;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PairBody {
    /// 用户输入的 6 位 PIN。
    pub pin: String,
    /// 用户给设备起的可读名（"以琳的 iPhone"）。
    pub device_name: String,
}

/// 200 响应体——客户端能看到自己拿到的 device_id（debug + 后续吊销定位用）。
/// **token 不进 body**——必须 Set-Cookie HttpOnly 防 XSS 偷走。
///
/// `Deserialize` 给 e2e 测试反序列化响应用——hand-written client 也用得上。
#[derive(Debug, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: String,
}

pub async fn pair(State(state): State<AppState>, Json(body): Json<PairBody>) -> Result<Response> {
    // 防 trivial input：handler 层做最低限度的格式校验（PIN 必须 6 位数字，
    // device_name 不空）。深层语义错（PIN 错/过期）由 PendingPairs::claim 给出。
    if body.pin.len() != 6 || !body.pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(Error::BadRequest("pin 必须是 6 位数字".into()));
    }
    let name = body.device_name.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("device_name 不能为空".into()));
    }

    state.im_auth.pairs.claim(&body.pin).map_err(|e| match e {
        ClaimError::Unknown | ClaimError::Expired | ClaimError::Locked => {
            Error::Unauthorized(format!("PIN 不可用：{e:?}"))
        }
    })?;

    // 设备 id 用 uuid v4，token 主体 + cookie + device_tokens 主键三处共用。
    let device_id = Uuid::new_v4().to_string();
    let claims = fresh_claims(device_id.clone(), name.to_string());
    let token = sign_token(&state.im_auth.secret, &claims)?;

    if let Some(devices) = &state.im_auth.devices {
        let now = Utc::now();
        let rec = DeviceRecord {
            token_id: device_id.clone(),
            device_name: name.to_string(),
            // 当前用全局 key 签发；本列存的是 base64url 形式的 secret 占位
            // （CLAUDE.md 决策：未来 per-device rotation 才用得到）。
            // WHY 不写真 key 进每行：global key 有自己的 ~/.fuxi/im_hmac.key 文件
            // 真相源；这里写 "global" 占位字符串避免泄漏 secret 到表里。
            hmac_secret: "global".to_string(),
            created_at: now,
            expires_at: claims.expires_at,
            revoked_at: None,
        };
        devices.insert(&rec).await?;
    }

    let cookie = build_set_cookie(&token, crate::auth::DEFAULT_TOKEN_TTL_DAYS);

    let body_json = Json(PairResponse {
        device_id: device_id.clone(),
    });

    let mut resp = (StatusCode::OK, body_json).into_response();
    // header::SET_COOKIE 直接 insert——可能被 axum 把 string 解析失败 unwrap，
    // 但 build_set_cookie 输出永远是 ASCII，安全。
    resp.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|e| Error::Internal(format!("set-cookie header 解析失败: {e}")))?,
    );
    let _ = COOKIE_NAME; // 防 unused import 警告——COOKIE_NAME 是公共 API 不能丢
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{HmacSecret, verify_token};
    use crate::pair::PendingPairs;
    use crate::state::ImAuth;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]).await;
        tokio::fs::write(path.join("README.md"), "seed")
            .await
            .unwrap();
        run_git(path, &["add", "-A"]).await;
        run_git(
            path,
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
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path.to_path_buf()));
        (dir, ws)
    }

    async fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    async fn build(im_auth: ImAuth) -> (tempfile::TempDir, Router, AppState) {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let (dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_im_auth(im_auth);
        let app = Router::new()
            .route("/api/auth/pair", post(pair))
            .with_state(state.clone());
        (dir, app, state)
    }

    #[tokio::test]
    async fn pair_with_valid_pin_signs_token_and_sets_cookie() {
        let secret = HmacSecret::from_string("test-key".into());
        let pairs = Arc::new(PendingPairs::new());
        let pin = pairs.start(crate::pair::DEFAULT_PIN_TTL);
        let im_auth = ImAuth {
            secret: Arc::new(secret),
            pairs: pairs.clone(),
            devices: None,
        };
        let (_dir, app, state) = build(im_auth).await;

        let body = serde_json::json!({ "pin": pin.as_str(), "device_name": "iPhone" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // 取 Set-Cookie，提 token，verify 一遍——白盒确认确实是 sign_token 出的活 token
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .expect("must have Set-Cookie")
            .to_str()
            .unwrap()
            .to_string();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Secure"));
        let token = set_cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches("fuxi_im_token=")
            .to_string();
        let claims = verify_token(&state.im_auth.secret, &token).expect("valid");
        assert_eq!(claims.name, "iPhone");

        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: PairResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(claims.device_id, parsed.device_id);
    }

    #[tokio::test]
    async fn pair_with_unknown_pin_returns_401() {
        let im_auth = ImAuth::ephemeral();
        let (_dir, app, _) = build(im_auth).await;
        let body = serde_json::json!({ "pin": "000000", "device_name": "x" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pair_with_bad_format_returns_400() {
        let im_auth = ImAuth::ephemeral();
        let (_dir, app, _) = build(im_auth).await;

        for (pin, name, expected) in [
            ("12345", "x", StatusCode::BAD_REQUEST),   // 5 位
            ("1234567", "x", StatusCode::BAD_REQUEST), // 7 位
            ("abcdef", "x", StatusCode::BAD_REQUEST),  // 非数字
            ("123456", "  ", StatusCode::BAD_REQUEST), // device_name 空白
        ] {
            let body = serde_json::json!({ "pin": pin, "device_name": name });
            let req = Request::builder()
                .method("POST")
                .uri("/api/auth/pair")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected, "pin={pin} name={name:?}");
        }
    }

    #[tokio::test]
    async fn pair_writes_device_record_when_store_present() {
        // 端到端：成功路径 + DeviceStore 注入 → device_tokens 表里能查到该 device_id
        let dir = tempfile::tempdir().expect("tmp");
        let db_path = dir.path().join("im.db");
        let pool = crate::db::init_at(&db_path).await.expect("init db");
        let store = crate::devices::DeviceStore::new(pool);

        let secret = HmacSecret::from_string("real-key".into());
        let pairs = Arc::new(PendingPairs::new());
        let pin = pairs.start(crate::pair::DEFAULT_PIN_TTL);
        let im_auth = ImAuth {
            secret: Arc::new(secret),
            pairs,
            devices: Some(store.clone()),
        };
        let (_ws_dir, app, _) = build(im_auth).await;

        let body = serde_json::json!({ "pin": pin.as_str(), "device_name": "Pixel" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/auth/pair")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: PairResponse = serde_json::from_slice(&bytes).unwrap();

        let row = store.get(&parsed.device_id).await.unwrap().expect("入库");
        assert_eq!(row.device_name, "Pixel");
        assert!(row.revoked_at.is_none());
    }
}
