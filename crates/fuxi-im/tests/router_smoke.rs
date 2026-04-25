//! `fuxi-im` 路由骨架冒烟测试。
//!
//! 只验"骨架装得上 + 三条最小契约成立"，不碰 β/γ/δ 的业务逻辑。
//! 三条契约：
//!   1. `GET /healthz` → 200 + body `"ok"`
//!   2. `GET /api/tasks?root=1` → 200 + JSON 数组（骨架阶段允许空数组）
//!   3. 不存在路由 → 404
//!
//! 后续 teammate 给 router 挂模块时，本文件不需要改——它锚的是不变契约。

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use fuxi_events::EventBus;
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
async fn build_router() -> (TempDir, axum::Router) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));
    let state = AppState::new(fuxi);
    (dir, router(state))
}

#[tokio::test]
async fn healthz_returns_ok() {
    let (_dir, app) = build_router().await;

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
    let (_dir, app) = build_router().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tasks?root=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert!(v.is_array(), "expect json array, got {v}");
}

#[tokio::test]
async fn unknown_route_is_404() {
    let (_dir, app) = build_router().await;

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/this-route-does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
