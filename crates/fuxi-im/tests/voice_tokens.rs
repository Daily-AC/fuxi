//! `/api/voice/tokens` 集成测——PWA 语音链路的鉴权桥。
//!
//! PWA 登录态是 HttpOnly cookie，JS 拿不到原始 token；而 asr/tts 代理要
//! Bearer/帧体 token、wake server 要独立预共享 token。本端点用 cookie 登录态
//! 换出这两颗 token，让前端语音模块能直连 `/api/asr`、`/api/tts`、`/wake/api/wake`。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use fuxi_events::EventBus;
use fuxi_im::auth::{COOKIE_NAME, HmacSecret, sign_token, verify_token};
use fuxi_im::devices::DeviceStore;
use fuxi_im::pair::PendingPairs;
use fuxi_im::state::ImAuth;
use fuxi_im::{AppState, router};
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::GitWorktreeWorkspace;
use std::path::PathBuf;
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

async fn build_app(wake_token_path: Option<PathBuf>) -> (TempDir, axum::Router, Arc<HmacSecret>) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));

    let secret = HmacSecret::from_string("voice-tokens-test-key".into());
    let secret_arc = Arc::new(secret);
    let im_auth = ImAuth {
        secret: secret_arc.clone(),
        pairs: Arc::new(PendingPairs::new()),
        devices: None::<DeviceStore>,
        password_path: None,
        login_guard: Arc::new(fuxi_im::lockout::LoginGuard::new()),
    };
    let mut state = AppState::new(fuxi).with_im_auth(im_auth);
    if let Some(p) = wake_token_path {
        state = state.with_wake_token_path(p);
    }
    let app = router(state);
    (dir, app, secret_arc)
}

fn signed_cookie(secret: &HmacSecret) -> String {
    let claims = fuxi_im::auth::fresh_claims("test-device".into(), "TestPhone".into());
    let token = sign_token(secret, &claims).expect("sign");
    format!("{COOKIE_NAME}={token}")
}

fn req(cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri("/api/voice/tokens");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    b.body(Body::empty()).unwrap()
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn without_cookie_returns_401() {
    let (_dir, app, _secret) = build_app(None).await;
    let resp = app.oneshot(req(None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn with_cookie_returns_fresh_im_token_verifiable_by_same_secret() {
    let wake_dir = tempfile::tempdir().unwrap();
    let wake_path = wake_dir.path().join("wake.token");
    tokio::fs::write(&wake_path, "deadbeef-wake-token\n")
        .await
        .unwrap();

    let (_dir, app, secret) = build_app(Some(wake_path)).await;
    let cookie = signed_cookie(&secret);
    let resp = app.oneshot(req(Some(&cookie))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    let im_token = json["im_token"].as_str().expect("im_token 字段");
    // 换出的 token 必须能被同一颗 HMAC key 验签——asr/tts 代理就是这么验的
    let claims = verify_token(&secret, im_token).expect("im_token 验签通过");
    assert!(claims.expires_at > chrono::Utc::now());
    // 文件里的换行要被 trim 掉——wake server 是逐字节比对
    assert_eq!(json["wake_token"].as_str(), Some("deadbeef-wake-token"));
}

#[tokio::test]
async fn missing_wake_token_file_degrades_to_null() {
    let wake_dir = tempfile::tempdir().unwrap();
    let absent = wake_dir.path().join("no-such.token");

    let (_dir, app, secret) = build_app(Some(absent)).await;
    let cookie = signed_cookie(&secret);
    let resp = app.oneshot(req(Some(&cookie))).await.unwrap();
    // 唤醒不可用不该拖垮按住说话 + TTS——im_token 照发，wake_token 为 null
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert!(json["im_token"].as_str().is_some());
    assert!(json["wake_token"].is_null());
}

#[tokio::test]
async fn unconfigured_wake_path_also_degrades_to_null() {
    let (_dir, app, secret) = build_app(None).await;
    let cookie = signed_cookie(&secret);
    let resp = app.oneshot(req(Some(&cookie))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert!(json["im_token"].as_str().is_some());
    assert!(json["wake_token"].is_null());
}
