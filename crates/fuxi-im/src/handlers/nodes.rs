//! `GET /api/nodes` —— dist topology + 每节点 worker 实例（β · #55，spec gap c）。
//!
//! 数据源：`AppState.nodes_provider` (trait，生产由 fuxi-cli 包
//! `Arc<DistController>` 注入)。`AppState.fuxi` 给 home 节点的 worker shelf 用。
//!
//! 503 路径：`nodes_provider` 未注入（测试 / smoke 场景）。production
//! `fuxi im start` 必注入；不注入 = 装配 bug。

use crate::error::{Error, Result};
use crate::nodes_provider::NodesResponse;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;

pub async fn list_nodes(State(state): State<AppState>) -> Result<Json<NodesResponse>> {
    let provider = state.nodes_provider.as_ref().ok_or_else(|| {
        Error::Unavailable("nodes_provider 未注入（dist controller 未启用？）".into())
    })?;
    let nodes = provider.list_nodes(&state.fuxi).await;
    Ok(Json(NodesResponse { nodes }))
}

#[cfg(test)]
mod tests {
    use crate::nodes_provider::{NodeView, NodesProvider, WorkerView};
    use crate::state::AppState;
    use async_trait::async_trait;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// stub provider 返预置 NodeView 列表——验 handler 把它包成 NodesResponse。
    struct StubProvider {
        canned: Vec<NodeView>,
    }

    #[async_trait]
    impl NodesProvider for StubProvider {
        async fn list_nodes(&self, _fuxi: &Fuxi) -> Vec<NodeView> {
            self.canned.clone()
        }
    }

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_buf = dir.path().to_path_buf();
        let _ = tokio::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&path_buf)
            .output()
            .await;
        tokio::fs::write(path_buf.join("README.md"), "x")
            .await
            .unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&path_buf)
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
            .current_dir(&path_buf)
            .output()
            .await;
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path_buf));
        (dir, ws)
    }

    async fn build_app(provider: Option<Arc<dyn NodesProvider>>) -> (tempfile::TempDir, Router) {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let mut state = AppState::new(fuxi);
        if let Some(p) = provider {
            state = state.with_nodes_provider(p);
        }
        let app = Router::new()
            .route("/api/nodes", get(super::list_nodes))
            .with_state(state);
        (dir, app)
    }

    #[tokio::test]
    async fn returns_503_when_provider_not_injected() {
        let (_dir, app) = build_app(None).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn returns_provider_list_wrapped_in_nodes_response() {
        let canned = vec![
            NodeView {
                node_id: "home".into(),
                tags: vec!["home".into(), "linux".into()],
                max_concurrency: 4,
                inflight_jobs: 1,
                heartbeat_lag_ms: Some(8230),
                online: true,
                registered_at_ms_ago: Some(60_000),
                workers: vec![WorkerView {
                    agent_id: "00000000-0000-0000-0000-000000000001".into(),
                    role: "luban".into(),
                    role_display: "鲁班".into(),
                    status: "busy".into(),
                }],
            },
            NodeView {
                node_id: "mac-local".into(),
                tags: vec!["local".into(), "mac".into()],
                max_concurrency: 2,
                inflight_jobs: 0,
                heartbeat_lag_ms: Some(4120),
                online: true,
                registered_at_ms_ago: Some(120_000),
                workers: vec![],
            },
        ];
        let provider = Arc::new(StubProvider {
            canned: canned.clone(),
        });
        let (_dir, app) = build_app(Some(provider)).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["nodes"].is_array());
        let nodes = v["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["node_id"], "home");
        assert_eq!(nodes[0]["tags"][0], "home");
        assert_eq!(nodes[0]["max_concurrency"], 4);
        assert_eq!(nodes[0]["inflight_jobs"], 1);
        assert_eq!(nodes[0]["online"], true);
        assert_eq!(nodes[0]["workers"][0]["role"], "luban");
        assert_eq!(nodes[0]["workers"][0]["role_display"], "鲁班");
        assert_eq!(nodes[0]["workers"][0]["status"], "busy");
        assert_eq!(nodes[1]["node_id"], "mac-local");
        // 远端节点 v1 workers 始终空
        assert!(nodes[1]["workers"].as_array().unwrap().is_empty());
    }
}
