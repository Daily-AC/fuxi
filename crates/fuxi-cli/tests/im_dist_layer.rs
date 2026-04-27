//! β · #54 集成测：fuxi-im axum app 同进程 merge dist HMAC router 后，
//! cookie auth layer 和 HMAC layer 互不干扰。
//!
//! 三条契约：
//! 1. `/api/*` 没 cookie → 401（cookie layer 拦了，HMAC layer 没误参与）
//! 2. `/dist/register` 没 HMAC headers → 401（HMAC layer 拦了，cookie layer 没拦）
//! 3. `/dist/register` 带合法 HMAC → 200/204（HMAC 放行进 handler，cookie layer 没拦）
//!
//! Why integration test：cookie layer 的 `is_exempt` 分支 (`!path.starts_with("/api/")`)
//! 是个 string check，理论上 `/dist/*` 路径不会被拦——本测验证这一假设在真 axum
//! merge 路径下成立，避免未来某次重构改了 exempt 逻辑造成 cross-talk。

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use fuxi_cli::im_dist::build_dist_layer;
use fuxi_events::EventBus;
use fuxi_im::auth::{COOKIE_NAME, HmacSecret as ImHmacSecret, fresh_claims, sign_token};
use fuxi_im::devices::DeviceStore;
use fuxi_im::pair::PendingPairs;
use fuxi_im::state::ImAuth;
use fuxi_im::{AppState, router as im_router};
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::GitWorktreeWorkspace;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn make_workspace() -> (TempDir, Arc<GitWorktreeWorkspace>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();
    let _ = tokio::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(path)
        .output()
        .await;
    tokio::fs::write(path.join("README.md"), "x").await.unwrap();
    let _ = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ])
        .current_dir(path)
        .output()
        .await;
    let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path.to_path_buf()));
    (dir, ws)
}

/// 装配跟生产 `fuxi im start` 等价的 axum app（IM router + dist HMAC router merge）。
/// 返回 (tempdir, app, valid_im_cookie)。
async fn build_merged_app() -> (TempDir, axum::Router, String) {
    // 干净 env：避免 env 残留影响 dist secret/token 解析
    unsafe {
        std::env::remove_var("FUXI_DIST_HMAC_SECRET");
        std::env::remove_var("FUXI_DIST_TOKEN");
    }

    let bus = EventBus::with_memory_store().await.expect("bus");
    let (ws_dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus.clone(), ws));

    // IM AppState
    let secret = ImHmacSecret::from_string("im-test-key".into());
    let secret_arc = Arc::new(secret);
    let im_auth = ImAuth {
        secret: secret_arc.clone(),
        pairs: Arc::new(PendingPairs::new()),
        devices: None::<DeviceStore>,
        password_path: None,
        login_guard: Arc::new(fuxi_im::lockout::LoginGuard::new()),
    };
    let im_state = AppState::new(fuxi).with_im_auth(im_auth);
    let im_router_built = im_router(im_state);

    // dist layer with self-generated secret/token under tempdir
    let dist_dir = TempDir::new().expect("dist tmp");
    let dist_layer = build_dist_layer(dist_dir.path(), bus)
        .await
        .expect("dist layer");

    // valid IM cookie
    let claims = fresh_claims("test-device".into(), "test".into());
    let token = sign_token(&secret_arc, &claims).expect("sign");
    let cookie = format!("{COOKIE_NAME}={token}");

    let merged = im_router_built.merge(dist_layer.router);
    // Hold both tempdirs alive in caller via the returned ws_dir tuple；dist_dir
    // 会在 mem 内被 drop——实际生产 dist_jobs.db 是长跑文件，本测内不需要持久。
    drop(dist_dir);
    (ws_dir, merged, cookie)
}

/// 契约 1：`/api/*` 缺 cookie → 401
#[tokio::test]
async fn api_route_without_cookie_returns_401_even_after_dist_merge() {
    let (_dir, app, _cookie) = build_merged_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tasks?root=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "cookie auth layer 应拦 /api/* 无 cookie 请求"
    );
}

/// 契约 2：`/dist/register` 缺 HMAC headers → 401（HMAC layer 拦）
#[tokio::test]
async fn dist_route_without_hmac_returns_401_not_cookie_401() {
    let (_dir, app, _cookie) = build_merged_app().await;
    // /dist/register 是 POST，body 必填
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/dist/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"node_id":"test","tags":[],"max_concurrency":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "HMAC layer 应拦 /dist/* 无签名请求"
    );
    // body 应是 HMAC layer 的 "unauthorized" 字面（dist_auth.rs::unauthorized()），
    // 而不是 cookie layer 的 JSON error envelope（fuxi-im::error）—— 证明拦的人对
    let body_bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert_eq!(body_str, "unauthorized", "应是 HMAC layer 拦的 plain text");
}

/// 契约 3：`/dist/*` 缺 cookie 但带合法 HMAC → 应过 cookie layer（被豁免），
/// 进 HMAC layer 验签（本测不构造合法签名，所以仍应是 401，但**body** 是
/// "unauthorized" 而非 cookie layer 的 JSON—— 证明 cookie layer 没拦）。
///
/// 注：构造合法 HMAC 签名走 dist_auth_client::sign_request；本测保留作 follow-up
/// （需要包 reqwest 整套 wire-level fixture），当前用反向断言（body 形态）即可
/// 证明 cookie layer 路径没误参与。
#[tokio::test]
async fn dist_route_bypasses_cookie_layer_proven_by_error_body_shape() {
    let (_dir, app, _cookie) = build_merged_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/dist/pull?node_id=test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // 401 from HMAC layer
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body_bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    // 关键：body 是 HMAC layer 的 "unauthorized"，不是 cookie layer 的 JSON
    // (`{"error":"unauthorized","message":"missing or invalid auth cookie"}`)
    assert_eq!(
        body_str, "unauthorized",
        "若 cookie layer 误参与，body 应是 JSON 而非 plain text"
    );
}

/// 反向契约：`/api/*` 带合法 cookie → 200（cookie layer 放行）。配 #1 一起证两层
/// 路径分明。
#[tokio::test]
async fn api_route_with_valid_cookie_returns_200_after_dist_merge() {
    let (_dir, app, cookie) = build_merged_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tasks?root=1")
                .header(axum::http::header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "合法 cookie 应通过 cookie layer，dist merge 不破坏既有路径"
    );
}
