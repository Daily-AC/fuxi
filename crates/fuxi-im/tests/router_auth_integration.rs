//! 端到端鉴权 layer 集成测——**走真 Router**，不直接调 `cookie_auth_layer` fn。
//!
//! 这是 #15 critical bug 的回归门禁：之前 layer 写好但**从没挂进 router.rs**，
//! 任何人能无 cookie 访问 `/api/*`。隔离单测（在 middleware.rs 里）通过的是
//! "layer 函数行为正确"——但抓不到"layer 没挂"这种装配 bug。
//!
//! 本测试**必须**在每条 PR 之前跑——所以放 tests/ 集成形态，cargo test 自动捞。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use fuxi_events::EventBus;
use fuxi_im::auth::{COOKIE_NAME, HmacSecret, TokenClaims, sign_token};
use fuxi_im::devices::DeviceStore;
use fuxi_im::pair::PendingPairs;
use fuxi_im::state::ImAuth;
use fuxi_im::{AppState, router};
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::GitWorktreeWorkspace;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::process::Command;
use tower::ServiceExt;

async fn make_workspace() -> (TempDir, Arc<GitWorktreeWorkspace>) {
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
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .await
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?} failed");
}

/// 装一份完整 router：注入已知 HMAC secret，调用方用同一 secret 签合法 token。
/// 返回 (router, secret) 让测试能签 cookie。
async fn build_app() -> (TempDir, axum::Router, Arc<HmacSecret>) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));

    // 注入显式 secret，签 token 时用同一个
    let secret = HmacSecret::from_string("integration-test-key".into());
    let secret_arc = Arc::new(secret);
    let im_auth = ImAuth {
        secret: secret_arc.clone(),
        pairs: Arc::new(PendingPairs::new()),
        devices: None::<DeviceStore>,
        password_path: None,
        login_guard: Arc::new(fuxi_im::lockout::LoginGuard::new()),
    };
    let state = AppState::new(fuxi).with_im_auth(im_auth);
    let app = router(state);
    (dir, app, secret_arc)
}

/// 用注入的 secret 签一份合法 cookie（默认 1 年过期）。
fn signed_cookie(secret: &HmacSecret) -> String {
    let claims = fuxi_im::auth::fresh_claims("test-device".into(), "TestPhone".into());
    let token = sign_token(secret, &claims).expect("sign");
    format!("{COOKIE_NAME}={token}")
}

/// 用注入的 secret 签一份**已过期** 1 秒的 cookie。
fn expired_cookie(secret: &HmacSecret) -> String {
    let claims = TokenClaims {
        device_id: "stale-device".into(),
        name: "Old".into(),
        expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
    };
    let token = sign_token(secret, &claims).expect("sign");
    format!("{COOKIE_NAME}={token}")
}

fn req(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::empty()).unwrap()
}

// ─── 核心契约：/api/* 强制带合法 cookie ─────────────────────────────────

#[tokio::test]
async fn api_tasks_without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    let resp = app.oneshot(req("/api/tasks?root=1", None)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "无 cookie 必须 401——之前 layer 没挂导致 200，是 #15 critical bug"
    );
}

#[tokio::test]
async fn api_tasks_with_valid_cookie_returns_200() {
    let (_dir, app, secret) = build_app().await;
    let cookie = signed_cookie(&secret);
    let resp = app
        .oneshot(req("/api/tasks?root=1", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_tasks_with_expired_cookie_returns_401() {
    let (_dir, app, secret) = build_app().await;
    let cookie = expired_cookie(&secret);
    let resp = app
        .oneshot(req("/api/tasks?root=1", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_tasks_with_tampered_cookie_returns_401() {
    let (_dir, app, secret) = build_app().await;
    let mut cookie = signed_cookie(&secret);
    // 翻转 cookie 末位字符——HMAC 必不匹配
    let last = cookie.pop().unwrap();
    cookie.push(if last == 'A' { 'B' } else { 'A' });
    let resp = app
        .oneshot(req("/api/tasks?root=1", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_tasks_with_wrong_secret_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    // 用别的 secret 签的 token——服务端用 build_app 里那个 secret 验，必不通
    let other = HmacSecret::from_string("attacker-key".into());
    let cookie = signed_cookie(&other);
    let resp = app
        .oneshot(req("/api/tasks?root=1", Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── 豁免路径：/api/auth/login + /api/auth/pair + /healthz ─────────────

#[tokio::test]
async fn healthz_is_exempt_no_cookie_required() {
    let (_dir, app, _secret) = build_app().await;
    let resp = app.oneshot(req("/healthz", None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// 区分"被 layer 拦下"vs"handler 自己返 401"：
/// - layer 拦下：body = `"unauthorized"`（middleware.rs unauthorized() 直接 plain text）
/// - handler 返：body 是 IM Error JSON {error,message}（IntoResponse）
async fn was_blocked_by_layer(resp: axum::response::Response) -> bool {
    if resp.status() != StatusCode::UNAUTHORIZED {
        return false;
    }
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    // layer 走的是 plain text "unauthorized"；handler 走的是 JSON
    body.trim() == "unauthorized"
}

#[tokio::test]
async fn api_auth_login_is_exempt_no_cookie_required() {
    // login 端点本身就是签发 cookie 的入口——不带 cookie 也得能走到 handler。
    // ephemeral state 没 password_path → handler 返 503 unavailable；不该被 layer 拦。
    let (_dir, app, _secret) = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "password": "anything",
                "device_name": "x"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        !was_blocked_by_layer(resp).await,
        "/api/auth/login 必须豁免 cookie 鉴权 layer，否则用户连登入都没法登"
    );
}

#[tokio::test]
async fn api_auth_pair_is_exempt_no_cookie_required() {
    // pair 端点是 fallback 入口——不带 cookie 也要能走到 handler。
    // PIN 不存在 → handler 自己返 401（带 JSON body），但**不是 layer 拦下的 401**
    // （那个会回 plain text "unauthorized"）。
    let (_dir, app, _secret) = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/pair")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "pin": "000000",
                "device_name": "x"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        !was_blocked_by_layer(resp).await,
        "/api/auth/pair 必须豁免 cookie 鉴权 layer"
    );
}

// ─── 其他 /api 端点也强制鉴权——抽样验证 layer 全网生效 ──────────────────

#[tokio::test]
async fn api_intervene_without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/intervene")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"text": "hi"})).unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_dispatch_without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/dispatch")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "title": "x",
                "description": "y"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_conv_ws_without_cookie_returns_401() {
    // WS upgrade 之前 layer 拦下——验"WS 端点同样受保护"。
    let (_dir, app, _secret) = build_app().await;
    let resp = app.oneshot(req("/api/conv", None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ─── #17 新加端点也强制鉴权 ────────────────────────────────────────────

#[tokio::test]
async fn api_conv_messages_without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    let resp = app
        .oneshot(req("/api/conv/messages?conv=xuannv", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_upload_without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/upload")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_uploads_get_without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app().await;
    let resp = app
        .oneshot(req("/api/uploads/anything", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
