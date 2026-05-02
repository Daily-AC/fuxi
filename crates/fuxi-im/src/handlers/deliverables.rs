//! `GET /api/deliverables` + `GET /api/deliverables/<project>/<task>/files/<name>`
//! —— PWA 收件箱数据源（Decision 22 phase 1）。
//!
//! 数据源：扫 `project_registry` 列出的全部 project，每个 project 走
//! `<projects_root>/<project>/deliverables/` 下所有 task 的 manifest.json。
//!
//! v1 形态：扁平列表，按 `produced_at` 倒序（新的在前）。后续若量级上去
//! 加索引 / pagination。
//!
//! 503 路径：registry 未注入 → 返 unavailable，PWA 应跳过收件箱标签。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use chrono::{DateTime, Utc};
use fuxi_core::{DeliverableFileMeta, DeliverableKind, ProjectId, TaskId};
use fuxi_workspace::{DeliverableManifest, FileSystemProjectRegistry};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncReadExt;

// PathBuf 在 tests 模块用——top-level 重新 import 给 tests use super::* 借
#[cfg(test)]
use std::path::PathBuf;

/// `/api/deliverables` 单条记录——一次 produce 调用 = 一条。
///
/// 同一 task 多次 produce 出多条（按时间序）。前端可按 task 聚合显示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverableEntry {
    pub project: ProjectId,
    pub task: TaskId,
    pub kind: DeliverableKind,
    pub files: Vec<DeliverableFileMeta>,
    pub produced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverablesResponse {
    pub deliverables: Vec<DeliverableEntry>,
}

/// `GET /api/deliverables` —— 全收件箱视图。
pub async fn list_deliverables(
    State(state): State<AppState>,
) -> Result<Json<DeliverablesResponse>> {
    let registry = state.project_registry.as_ref().ok_or_else(|| {
        Error::Unavailable("project_registry 未注入（fuxi im start 未配置？）".into())
    })?;
    let mut all = collect_all_deliverables(registry)
        .await
        .map_err(|e| Error::Internal(format!("扫 deliverables 失败: {e}")))?;
    // 倒序：新的在前
    all.sort_by(|a, b| b.produced_at.cmp(&a.produced_at));
    Ok(Json(DeliverablesResponse { deliverables: all }))
}

/// `GET /api/deliverables/:project/:task/files/:name` —— 下载交付文件。
///
/// 路径校验：project 必须已注册 + task 在 deliverables/ 下有 bucket +
/// 文件名不含 `..`（防 path traversal）。否则 404。
///
/// 不要求文件 sha256 重算——manifest 写时已算，本端点是 fast path。
pub async fn download_file(
    State(state): State<AppState>,
    AxumPath((project_raw, task_raw, name)): AxumPath<(String, String, String)>,
) -> std::result::Result<Response, Error> {
    let registry = state
        .project_registry
        .as_ref()
        .ok_or_else(|| Error::Unavailable("project_registry 未注入".into()))?;

    // 1. project 校验 + lookup
    let project_id = ProjectId::new(project_raw.clone())
        .map_err(|_| Error::NotFound(format!("project {project_raw}")))?;
    let project = registry
        .get(&project_id)
        .await
        .map_err(|e| Error::Internal(format!("project lookup 失败: {e}")))?
        .ok_or_else(|| Error::NotFound(format!("project {project_id}")))?;

    // 2. 防 path traversal：文件名不能含 `/` 或 `..`
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(Error::BadRequest(format!("非法文件名 {name:?}")));
    }
    // task 同样校验：纯 ASCII / 标准 task-<uuid> 形态
    if !task_raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(Error::BadRequest(format!("非法 task id {task_raw:?}")));
    }

    // 3. 拼路径
    let bucket = registry
        .root()
        .join(project.id.as_str())
        .join("deliverables")
        .join(&task_raw);
    let file_path = bucket.join(&name);

    // 4. 文件必须存在 + 必须在 bucket 内（再防 traversal）
    let canon_bucket = match bucket.canonicalize() {
        Ok(p) => p,
        Err(_) => return Err(Error::NotFound(format!("task {task_raw} 无 bucket"))),
    };
    let canon_file = file_path
        .canonicalize()
        .map_err(|_| Error::NotFound(format!("file {name}")))?;
    if !canon_file.starts_with(&canon_bucket) {
        return Err(Error::BadRequest("file outside bucket".into()));
    }
    if !canon_file.is_file() {
        return Err(Error::NotFound(format!("file {name}")));
    }

    // 5. 读文件返
    let mut file = fs::File::open(&canon_file)
        .await
        .map_err(|e| Error::Internal(format!("打开文件失败: {e}")))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .await
        .map_err(|e| Error::Internal(format!("读文件失败: {e}")))?;

    // content-type：粗略按扩展名；前端预览主要看 md / csv / txt / png 等
    let ct = guess_content_type(&name);
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(Body::from(bytes))
        .map_err(|e| Error::Internal(format!("构造 response 失败: {e}")))?;
    Ok(resp)
}

fn guess_content_type(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".md") {
        "text/markdown; charset=utf-8"
    } else if lower.ends_with(".txt") {
        "text/plain; charset=utf-8"
    } else if lower.ends_with(".csv") {
        "text/csv; charset=utf-8"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".pdf") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

async fn collect_all_deliverables(
    registry: &FileSystemProjectRegistry,
) -> std::result::Result<Vec<DeliverableEntry>, std::io::Error> {
    let mut out = Vec::new();
    let projects = registry
        .list()
        .await
        .map_err(|e| std::io::Error::other(format!("project list: {e}")))?;
    for project in projects {
        let deliv_root = registry
            .root()
            .join(project.id.as_str())
            .join("deliverables");
        if !deliv_root.exists() {
            continue;
        }
        let mut tasks = match fs::read_dir(&deliv_root).await {
            Ok(d) => d,
            Err(_) => continue,
        };
        while let Some(entry) = tasks.next_entry().await? {
            let manifest_path = entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let bytes = match fs::read(&manifest_path).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let manifest: DeliverableManifest = match serde_json::from_slice(&bytes) {
                Ok(m) => m,
                Err(_) => continue, // 损坏 manifest 跳过，不让单条脏数据搞挂全部
            };
            for entry in manifest.entries {
                out.push(DeliverableEntry {
                    project: manifest.project.clone(),
                    task: manifest.task,
                    kind: entry.kind,
                    files: entry.files,
                    produced_at: entry.produced_at,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::Request;
    use axum::routing::get as axum_get;
    use fuxi_core::DeliverableKind;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::{DeliverablesManager, FileSystemProjectRegistry, GitWorktreeWorkspace};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::process::Command;
    use tower::ServiceExt;

    async fn make_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await
                .unwrap();
        }
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await
            .unwrap();
        Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "x"])
            .output()
            .await
            .unwrap();
        (dir, path)
    }

    async fn make_app_with_data() -> (
        TempDir,
        Router,
        FileSystemProjectRegistry,
        ProjectId,
        TaskId,
    ) {
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());

        // 注册 project
        let (_repo_td, repo) = make_repo().await;
        let project = registry.add(repo, Some("erp".into()), None).await.unwrap();

        // 落一条 deliverable
        let task = TaskId::new();
        let mgr = DeliverablesManager::new(project.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("report.md");
        tokio::fs::write(&src, "# 报告\n内容").await.unwrap();
        mgr.produce(task, DeliverableKind::ResearchSummary, &[src])
            .await
            .unwrap();

        // 装 router
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(ws_dir.path()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/deliverables", axum_get(list_deliverables))
            .route(
                "/api/deliverables/{project}/{task}/files/{name}",
                axum_get(download_file),
            )
            .with_state(state);
        (registry_root, app, registry, project.id, task)
    }

    #[tokio::test]
    async fn list_returns_produced_entry() {
        let (_root, app, _r, _p, task) = make_app_with_data().await;
        let req = Request::builder()
            .uri("/api/deliverables")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entries = v["deliverables"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["project"], "erp");
        // TaskId serde 是 transparent UUID（无 task- 前缀）；只有 Display 时加前缀
        assert_eq!(entries[0]["task"], task.0.to_string());
        assert_eq!(entries[0]["kind"], "research_summary");
        let files = entries[0]["files"].as_array().unwrap();
        assert_eq!(files[0]["name"], "report.md");
    }

    #[tokio::test]
    async fn list_returns_503_when_registry_missing() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(ws_dir.path()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi);
        let app = Router::new()
            .route("/api/deliverables", axum_get(list_deliverables))
            .with_state(state);
        let req = Request::builder()
            .uri("/api/deliverables")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn download_returns_file_bytes() {
        let (_root, app, _r, project, task) = make_app_with_data().await;
        let req = Request::builder()
            .uri(format!(
                "/api/deliverables/{project}/{task}/files/report.md"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string());
        assert!(ct.unwrap_or_default().contains("text/markdown"));
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("# 报告"));
    }

    #[tokio::test]
    async fn download_rejects_path_traversal() {
        let (_root, app, _r, project, task) = make_app_with_data().await;
        // axum 路由对 path 带 `/` 通常会先做路径分段 → 404，但带 `..` 入参手 craft
        let req = Request::builder()
            .uri(format!(
                "/api/deliverables/{project}/{task}/files/..%2Fmanifest.json"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // 应被 BadRequest 或 NotFound 拒绝
        assert_ne!(resp.status(), StatusCode::OK, "path traversal 不该 200");
    }

    #[tokio::test]
    async fn download_404_for_unknown_project() {
        let (_root, app, _r, _p, task) = make_app_with_data().await;
        let req = Request::builder()
            .uri(format!(
                "/api/deliverables/no-such-project/{task}/files/report.md"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
