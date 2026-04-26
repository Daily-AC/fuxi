//! `fuxi-im` 路由骨架冒烟测试。
//!
//! 只验"骨架装得上 + 三条最小契约成立"，不碰 β/γ/δ 的业务逻辑。
//! 三条契约：
//!   1. `GET /healthz` → 200 + body `"ok"`（layer 豁免）
//!   2. `GET /api/tasks?root=1` 带合法 cookie → 200 + `{ running, completed }` JSON
//!   3. 不存在路由 → 404
//!
//! #15 之后：`/api/*` 在 router 上强制带合法 cookie——本测试用注入 secret + 手签
//! token 模拟登入后的请求。详细鉴权契约见 `tests/router_auth_integration.rs`。

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use fuxi_events::EventBus;
use fuxi_im::auth::{COOKIE_NAME, HmacSecret, fresh_claims, sign_token};
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

/// 造一份能让 `Fuxi::new` 接生的最小 workspace——空仓库 + main 分支 + 首个 commit。
/// 与 `crates/fuxi-orchestrator/tests/dispatch.rs::make_workspace` 同形（路径独立、互不影响）。
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

/// 装配一份骨架 router——内置一个真的 `Fuxi`（零 worker 启动），保证签名兼容。
/// 返回 (tempdir, router, cookie_for_authenticated_request)。cookie 用注入的 secret
/// 签出来——给 `/api/*` 测试用。
async fn build_router() -> (TempDir, axum::Router, String) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));
    let secret = HmacSecret::from_string("smoke-key".into());
    let secret_arc = Arc::new(secret);
    let im_auth = ImAuth {
        secret: secret_arc.clone(),
        pairs: Arc::new(PendingPairs::new()),
        devices: None::<DeviceStore>,
        password_path: None,
        login_guard: Arc::new(fuxi_im::lockout::LoginGuard::new()),
    };
    let state = AppState::new(fuxi).with_im_auth(im_auth);
    let claims = fresh_claims("smoke-device".into(), "smoke".into());
    let token = sign_token(&secret_arc, &claims).expect("sign");
    let cookie = format!("{COOKIE_NAME}={token}");
    (dir, router(state), cookie)
}

#[tokio::test]
async fn healthz_returns_ok() {
    let (_dir, app, _cookie) = build_router().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&bytes[..], b"ok");
}

#[tokio::test]
async fn api_tasks_root_returns_json_array() {
    let (_dir, app, cookie) = build_router().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tasks?root=1")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    // β · #21 后契约改成 `{ running: [], completed: [] }`
    assert!(v.is_object(), "expect json object, got {v}");
    assert!(v["running"].is_array(), "running 应是数组：{v}");
    assert!(v["completed"].is_array(), "completed 应是数组：{v}");
}

#[tokio::test]
async fn unknown_route_is_404() {
    let (_dir, app, _cookie) = build_router().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/this-route-does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // 404 而非 401——middleware `!path.starts_with("/api/")` 分支放行非 /api 路径
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
