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
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use fuxi_core::ProjectId;
use fuxi_workspace::PersistentSandboxManager;
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

/// `POST /api/projects` 请求体——PWA「+ 注册项目」按钮的载荷。
#[derive(Debug, Clone, Deserialize)]
pub struct AddProjectRequest {
    /// 用户真项目目录（绝对路径，必须已是 git repo）。
    pub canonical_path: PathBuf,
    /// 可选 slug。不传从 basename 派生；非 ASCII basename 派生失败 → 必须传。
    #[serde(default)]
    pub name: Option<String>,
    /// 可选默认 branch。不传默认 `main`。
    #[serde(default)]
    pub default_branch: Option<String>,
}

/// `POST /api/projects` —— 注册一个项目。
///
/// 业务规则：
/// - canonical 必须存在 + 是 git repo（含 `.git`）→ 否则 400/404
/// - id 重名 → 409
/// - 成功 200 返 ProjectView（同 GET 单条形态）
///
/// **不**支持远端 path（需 SSH）—— 假定 fuxi-im 跟用户 repo 同机器。跨机
/// repo 后续走 dist worker 路径，不在本端点 scope。
pub async fn add_project(
    State(state): State<AppState>,
    Json(req): Json<AddProjectRequest>,
) -> Result<(StatusCode, Json<ProjectView>)> {
    let registry = state.project_registry.as_ref().ok_or_else(|| {
        Error::Unavailable("project_registry 未注入（fuxi im start 未配置？）".into())
    })?;
    let project = registry
        .add(req.canonical_path, req.name, req.default_branch)
        .await
        .map_err(|e| match e {
            fuxi_workspace::WorkspaceError::AlreadyExists(_) => {
                Error::Conflict(format!("project 已存在: {e}"))
            }
            fuxi_workspace::WorkspaceError::NotAGitRepo(p) => {
                Error::BadRequest(format!("路径不是 git repo: {}", p.display()))
            }
            other => Error::BadRequest(format!("注册失败: {other}")),
        })?;
    Ok((
        StatusCode::CREATED,
        Json(ProjectView {
            id: project.id,
            canonical_path: project.canonical_path,
            default_branch: project.default_branch,
            created_at: project.created_at,
        }),
    ))
}

/// `DELETE /api/projects/{id}` —— 删项目。
///
/// **destructive**：sandboxes / ephemeral / archive / deliverables 一并清掉。
/// 调用方负责确认（PWA 二次确认弹窗）。
/// 不存在 → 404；id 非法 slug → 400。
pub async fn remove_project(
    State(state): State<AppState>,
    AxumPath(id_raw): AxumPath<String>,
) -> Result<StatusCode> {
    let registry = state
        .project_registry
        .as_ref()
        .ok_or_else(|| Error::Unavailable("project_registry 未注入".into()))?;
    let id = ProjectId::new(id_raw.clone())
        .map_err(|e| Error::BadRequest(format!("非法 project id {id_raw:?}: {e}")))?;
    if registry
        .get(&id)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
        .is_none()
    {
        return Err(Error::NotFound(format!("project {id}")));
    }
    registry
        .remove(&id)
        .await
        .map_err(|e| Error::Internal(format!("删项目失败: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `/api/projects/{id}/sandboxes` 单条响应——展示 L3 持久 sandbox 给 PWA。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxView {
    /// L3 sandbox 关联的门客 role，比如 "luban"。
    pub role: String,
    /// `<project>/L3/<role>` 跨事件关联键（同 EventBus WorkspaceId 形态）。
    pub workspace_id: String,
    /// sandbox 在磁盘上的绝对路径——用户可 cd / IDE 打开。
    pub path: PathBuf,
    /// 长期 branch 名，比如 `luban/erp-main`。
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxesResponse {
    pub sandboxes: Vec<SandboxView>,
}

/// `GET /api/projects/{id}/sandboxes` —— 列出某项目的所有 L3 持久 sandbox。
///
/// 数据源：扫 `<projects_root>/<project>/sandboxes/` 子目录。每个子目录名 = role。
/// 不查 git workspace 也不动 fuxi shelf——纯 fs view，跟 spawn 解耦。
pub async fn list_sandboxes(
    State(state): State<AppState>,
    AxumPath(id_raw): AxumPath<String>,
) -> Result<Json<SandboxesResponse>> {
    let registry = state
        .project_registry
        .as_ref()
        .ok_or_else(|| Error::Unavailable("project_registry 未注入".into()))?;
    let id = ProjectId::new(id_raw.clone())
        .map_err(|e| Error::BadRequest(format!("非法 project id {id_raw:?}: {e}")))?;
    let project = registry
        .get(&id)
        .await
        .map_err(|e| Error::Internal(format!("project lookup 失败: {e}")))?
        .ok_or_else(|| Error::NotFound(format!("project {id}")))?;

    let mgr = PersistentSandboxManager::new(project.clone(), registry.root());
    let handles = mgr
        .list()
        .await
        .map_err(|e| Error::Internal(format!("list sandboxes 失败: {e}")))?;
    let sandboxes = handles
        .into_iter()
        .map(|h| SandboxView {
            role: h.role,
            workspace_id: h.workspace_id.0,
            path: h.sandbox_path,
            branch: h.branch,
        })
        .collect();
    Ok(Json(SandboxesResponse { sandboxes }))
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
    async fn add_project_creates_then_lists() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo_path) = make_workspace_repo().await;

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/projects", axum_get(list_projects).post(add_project))
            .with_state(state);

        // POST 注册
        let body = serde_json::json!({
            "canonical_path": repo_path.to_string_lossy(),
            "name": "from-pwa",
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // GET 应能看到
        let req2 = Request::builder()
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let bytes = to_bytes(resp2.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let projects = v["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["id"], "from-pwa");
    }

    #[tokio::test]
    async fn add_project_409_on_duplicate() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo_path) = make_workspace_repo().await;
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        registry
            .add(repo_path.clone(), Some("dup".into()), None)
            .await
            .unwrap();

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/projects", axum_get(list_projects).post(add_project))
            .with_state(state);

        let body = serde_json::json!({
            "canonical_path": repo_path.to_string_lossy(),
            "name": "dup",
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn add_project_400_on_non_git_path() {
        let registry_root = tempfile::tempdir().unwrap();
        let non_git = tempfile::tempdir().unwrap();

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/projects", axum_get(list_projects).post(add_project))
            .with_state(state);

        let body = serde_json::json!({
            "canonical_path": non_git.path().to_string_lossy(),
            "name": "nogit",
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_sandboxes_returns_existing_sandboxes() {
        use fuxi_workspace::PersistentSandboxManager;

        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_repo_td, repo_path) = make_workspace_repo().await;
        let project = registry
            .add(repo_path, Some("erp".into()), None)
            .await
            .unwrap();

        // 起两个 L3 sandbox（luban + pusong）
        let mgr = PersistentSandboxManager::new(project.clone(), registry.root());
        mgr.get_or_create("luban").await.unwrap();
        mgr.get_or_create("pusong").await.unwrap();

        // 起 router
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/projects/{id}/sandboxes", axum_get(list_sandboxes))
            .with_state(state);

        let req = Request::builder()
            .uri("/api/projects/erp/sandboxes")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let sandboxes = v["sandboxes"].as_array().unwrap();
        assert_eq!(sandboxes.len(), 2);
        // 字典序：luban < pusong
        assert_eq!(sandboxes[0]["role"], "luban");
        assert_eq!(sandboxes[0]["workspace_id"], "erp/L3/luban");
        assert_eq!(sandboxes[0]["branch"], "luban/erp-main");
        assert_eq!(sandboxes[1]["role"], "pusong");
    }

    #[tokio::test]
    async fn list_sandboxes_404_for_unknown_project() {
        let registry_root = tempfile::tempdir().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/projects/{id}/sandboxes", axum_get(list_sandboxes))
            .with_state(state);
        let req = Request::builder()
            .uri("/api/projects/nope/sandboxes")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_project_removes_then_404() {
        use axum::routing::delete as axum_delete;

        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo_path) = make_workspace_repo().await;
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        registry
            .add(repo_path, Some("doomed".into()), None)
            .await
            .unwrap();

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/projects/{id}", axum_delete(remove_project))
            .with_state(state);

        // 第一次 DELETE → 204
        let req = Request::builder()
            .method("DELETE")
            .uri("/api/projects/doomed")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // 第二次 → 404（已删）
        let req2 = Request::builder()
            .method("DELETE")
            .uri("/api/projects/doomed")
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_FOUND);
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
