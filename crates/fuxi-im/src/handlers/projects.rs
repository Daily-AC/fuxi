//! `GET /api/projects` —— Project 注册表读视图（Decision 21 phase 1）。
//!
//! 数据源：`AppState.project_registry`，production 由 `fuxi im start` 注入
//! `FileSystemProjectRegistry::with_default_root()`（落 `~/.fuxi/projects/`）。
//!
//! 503 路径：注册表未注入（测试 / smoke 场景）。production 必注入；不注入 = 装配 bug。
//!
//! v1 只读：注册 / 删 项目走 CLI `fuxi project add|rm`。PWA 后续若加 GUI
//! 注册流再加 POST/DELETE。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use fuxi_core::ProjectId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `/api/projects` 单条响应——wire 形态。
///
/// 跟 `fuxi_core::Project` 字段一致，但**单独的 wire 类型**让前端跟核心
/// vocabulary 解耦：日后 Project 加内部字段（owner / quota 等）也不必同步
/// 暴露给前端。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectView {
    pub id: ProjectId,
    pub canonical_path: PathBuf,
    pub default_branch: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectsResponse {
    pub projects: Vec<ProjectView>,
}

pub async fn list_projects(State(state): State<AppState>) -> Result<Json<ProjectsResponse>> {
    let registry = state.project_registry.as_ref().ok_or_else(|| {
        Error::Unavailable("project_registry 未注入（fuxi im start 未配置？）".into())
    })?;
    let raw = registry
        .list()
        .await
        .map_err(|e| Error::Internal(format!("project list 失败: {e}")))?;
    let projects: Vec<ProjectView> = raw
        .into_iter()
        .map(|p| ProjectView {
            id: p.id,
            canonical_path: p.canonical_path,
            default_branch: p.default_branch,
            created_at: p.created_at,
        })
        .collect();
    Ok(Json(ProjectsResponse { projects }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get as axum_get;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::{FileSystemProjectRegistry, GitWorktreeWorkspace};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    async fn make_workspace() -> (TempDir, Arc<GitWorktreeWorkspace>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let _ = tokio::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&path)
            .output()
            .await;
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&path)
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
                "x",
            ])
            .current_dir(&path)
            .output()
            .await;
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path));
        (dir, ws)
    }

    async fn make_app(with_registry: bool) -> (TempDir, Router, Option<TempDir>) {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let mut state = AppState::new(fuxi);
        let registry_dir = if with_registry {
            let registry_root = tempfile::tempdir().unwrap();
            let registry = FileSystemProjectRegistry::new(registry_root.path());
            state = state.with_project_registry(registry);
            Some(registry_root)
        } else {
            None
        };
        let app = Router::new()
            .route("/api/projects", axum_get(list_projects))
            .with_state(state);
        (ws_dir, app, registry_dir)
    }

    #[tokio::test]
    async fn returns_503_when_registry_not_injected() {
        let (_ws_dir, app, _) = make_app(false).await;
        let req = Request::builder()
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn returns_empty_list_when_no_projects() {
        let (_ws_dir, app, _registry_dir) = make_app(true).await;
        let req = Request::builder()
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["projects"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn returns_registered_projects() {
        let (_ws_dir, _, registry_dir) = make_app(true).await;
        let registry_dir = registry_dir.unwrap();
        let registry = FileSystemProjectRegistry::new(registry_dir.path());

        // 注两个 project（每个一个独立的 git repo）
        let (_repo1, repo_path1) = make_workspace_repo().await;
        let (_repo2, repo_path2) = make_workspace_repo().await;
        registry
            .add(repo_path1, Some("erp".into()), None)
            .await
            .unwrap();
        registry
            .add(repo_path2, Some("fuxi-test".into()), None)
            .await
            .unwrap();

        // 重建 state + router 让它指向同一 registry root
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir2, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_dir.path()));
        let app = Router::new()
            .route("/api/projects", axum_get(list_projects))
            .with_state(state);

        let req = Request::builder()
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let projects = v["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 2);
        // 字典序：erp < fuxi-test
        assert_eq!(projects[0]["id"], "erp");
        assert_eq!(projects[1]["id"], "fuxi-test");
        assert_eq!(projects[0]["default_branch"], "main");
    }

    /// 给 returns_registered_projects 测试用——单独建一个 git repo 而非复用
    /// AppState 自己的 ws，这样 registry 的 canonical_path 跟 ws.repo_root 不冲突。
    async fn make_workspace_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await;
        }
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "x"])
            .output()
            .await;
        (dir, path)
    }
}
