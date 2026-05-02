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
use fuxi_core::{
    DeliverableFileMeta, DeliverableKind, Event, EventKind, EventMeta, ProjectId, TaskId,
};
use fuxi_workspace::{DeliverableManifest, FileSystemProjectRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::fs;
use tokio::io::AsyncReadExt;

/// 单 task 当前的处理状态（Decision 22 phase 2 加）—— `/api/deliverables`
/// 列表项 + 单 task 视图都返此状态字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverableStatus {
    /// 未处理——manifest 落了但用户没 accept/reject。
    Pending,
    Accepted,
    Rejected,
    Expired,
}

/// `/api/deliverables` 单条记录——一次 produce 调用 = 一条。
///
/// 同一 task 多次 produce 出多条（按时间序）。前端可按 task 聚合显示。
/// `status` 由扫描 events.db 里的 DeliverableAccepted/Rejected/Expired 事件
/// 推出——按 task 维度，最近一条状态变化事件赢。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverableEntry {
    pub project: ProjectId,
    pub task: TaskId,
    pub kind: DeliverableKind,
    pub files: Vec<DeliverableFileMeta>,
    pub produced_at: DateTime<Utc>,
    /// Decision 22 phase 2 加。任一未处理 entry 默认 `pending`；同 task 的多条
    /// entries 共用同一 task-级 status（accept/reject 是 task 维度的，不分单条）。
    pub status: DeliverableStatus,
}

/// `POST /api/deliverables/{project}/{task}/accept` 请求体。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AcceptRequest {
    /// 用户在 PWA 指定的目标路径——可选；不传 = inbox 模式（文件留在 deliverables/）。
    /// v1 phase 2 仅记事件，不真实拷贝文件（拷贝逻辑留 phase 3）。
    #[serde(default)]
    pub accepted_to: Option<PathBuf>,
}

/// `POST /api/deliverables/{project}/{task}/reject` 请求体。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RejectRequest {
    #[serde(default)]
    pub reason: Option<String>,
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
    // 叠加 task 维度的 accept/reject/expired 状态。
    let status_by_task = scan_task_status(&state).await?;
    for entry in &mut all {
        if let Some(s) = status_by_task.get(&entry.task) {
            entry.status = *s;
        }
    }
    // 倒序：新的在前
    all.sort_by(|a, b| b.produced_at.cmp(&a.produced_at));
    Ok(Json(DeliverablesResponse { deliverables: all }))
}

/// `POST /api/deliverables/{project}/{task}/accept` —— 标记交付已接收。
///
/// v1 phase 2：仅 publish DeliverableAccepted 事件。文件**不**真实拷贝到
/// `accepted_to`——拷贝逻辑留 phase 3，避免本期跨大量 path 边界 case 验证。
/// 用户当前可手动 cp 或 PWA 下载链接拿。
pub async fn accept_deliverable(
    State(state): State<AppState>,
    AxumPath((project_raw, task_raw)): AxumPath<(String, String)>,
    Json(body): Json<AcceptRequest>,
) -> Result<StatusCode> {
    let (project_id, task_id) = parse_project_and_task(&state, &project_raw, &task_raw).await?;
    let mut meta = EventMeta::now();
    meta.task = Some(task_id);
    let _ = state.fuxi.bus().publish(Event {
        meta,
        kind: EventKind::DeliverableAccepted {
            task: task_id,
            accepted_to: body.accepted_to,
        },
    });
    let _ = project_id; // project 校验用，事件本身不带 project（meta.task 足够）
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/deliverables/{project}/{task}/reject`。
pub async fn reject_deliverable(
    State(state): State<AppState>,
    AxumPath((project_raw, task_raw)): AxumPath<(String, String)>,
    Json(body): Json<RejectRequest>,
) -> Result<StatusCode> {
    let (_project, task_id) = parse_project_and_task(&state, &project_raw, &task_raw).await?;
    let mut meta = EventMeta::now();
    meta.task = Some(task_id);
    let _ = state.fuxi.bus().publish(Event {
        meta,
        kind: EventKind::DeliverableRejected {
            task: task_id,
            reason: body.reason,
        },
    });
    Ok(StatusCode::NO_CONTENT)
}

/// 校验 + 解析路径段为 (ProjectId, TaskId)。统一 accept/reject 两端点用。
///
/// 失败映射：project 不存在 / task 格式错 → 404；id 非法 slug → 400。
async fn parse_project_and_task(
    state: &AppState,
    project_raw: &str,
    task_raw: &str,
) -> Result<(ProjectId, TaskId)> {
    let registry = state
        .project_registry
        .as_ref()
        .ok_or_else(|| Error::Unavailable("project_registry 未注入".into()))?;
    let project_id = ProjectId::new(project_raw.to_string())
        .map_err(|_| Error::NotFound(format!("project {project_raw}")))?;
    if registry
        .get(&project_id)
        .await
        .map_err(|e| Error::Internal(format!("project lookup 失败: {e}")))?
        .is_none()
    {
        return Err(Error::NotFound(format!("project {project_id}")));
    }
    // task path 通常是 `task-<uuid>`，但也兼容裸 uuid
    let trimmed = task_raw.strip_prefix("task-").unwrap_or(task_raw);
    let uuid = uuid::Uuid::from_str(trimmed)
        .map_err(|_| Error::BadRequest(format!("task id 不是合法 UUID: {task_raw}")))?;
    Ok((project_id, TaskId::from(uuid)))
}

/// 扫 events.db 里的 DeliverableAccepted / Rejected / Expired——按 task 维度推出
/// 当前 status。最近一条事件赢（按 rowid 升序遍历，覆盖 update 取末态）。
async fn scan_task_status(state: &AppState) -> Result<HashMap<TaskId, DeliverableStatus>> {
    let store = state.fuxi.bus().store();
    let mut out = HashMap::new();
    for kind_tag in [
        "deliverable_accepted",
        "deliverable_rejected",
        "deliverable_expired",
    ] {
        let events = store
            .events_by_kind(kind_tag)
            .await
            .map_err(|e| Error::Internal(format!("scan {kind_tag} 失败: {e}")))?;
        for ev in events {
            let (task, status) = match ev.kind {
                EventKind::DeliverableAccepted { task, .. } => (task, DeliverableStatus::Accepted),
                EventKind::DeliverableRejected { task, .. } => (task, DeliverableStatus::Rejected),
                EventKind::DeliverableExpired { task } => (task, DeliverableStatus::Expired),
                _ => continue,
            };
            out.insert(task, status); // 末写胜（events_by_kind 返 rowid 升序）
        }
    }
    Ok(out)
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
                    status: DeliverableStatus::Pending,
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
    async fn accept_publishes_event_and_status_changes() {
        use axum::routing::post as axum_post;

        // 不复用 make_app_with_data——本 test 要装两端点（list + accept）共用同一
        // state，重建更直接。
        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_repo_td, repo) = make_repo().await;
        let project_obj = registry.add(repo, Some("erp".into()), None).await.unwrap();

        // 落一条 deliverable
        let task = TaskId::new();
        let mgr = DeliverablesManager::new(project_obj.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("report.md");
        tokio::fs::write(&src, "x").await.unwrap();
        mgr.produce(task, DeliverableKind::ResearchSummary, &[src])
            .await
            .unwrap();

        let bus = EventBus::with_memory_store().await.unwrap();
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(ws_dir.path()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/deliverables", axum_get(list_deliverables))
            .route(
                "/api/deliverables/{project}/{task}/accept",
                axum_post(accept_deliverable),
            )
            .with_state(state);

        // 1. POST accept
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/deliverables/erp/task-{}/accept", task.0))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"accepted_to":"/tmp/keep.md"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        // 给 EventStore writer 一点 flush 时间（broadcast → store 异步）
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 2. GET 列表 status 应是 accepted
        let req2 = Request::builder()
            .uri("/api/deliverables")
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        let bytes = to_bytes(resp2.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let entries = v["deliverables"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["status"], "accepted");
    }

    #[tokio::test]
    async fn reject_publishes_event_and_status_changes() {
        use axum::routing::post as axum_post;

        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_repo_td, repo) = make_repo().await;
        let project_obj = registry.add(repo, Some("erp".into()), None).await.unwrap();

        let task = TaskId::new();
        let mgr = DeliverablesManager::new(project_obj.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("x.md");
        tokio::fs::write(&src, "y").await.unwrap();
        mgr.produce(task, DeliverableKind::ResearchSummary, &[src])
            .await
            .unwrap();

        let bus = EventBus::with_memory_store().await.unwrap();
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(ws_dir.path()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route("/api/deliverables", axum_get(list_deliverables))
            .route(
                "/api/deliverables/{project}/{task}/reject",
                axum_post(reject_deliverable),
            )
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/deliverables/erp/task-{}/reject", task.0))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"reason":"内容不对"}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let req2 = Request::builder()
            .uri("/api/deliverables")
            .body(Body::empty())
            .unwrap();
        let resp2 = app.oneshot(req2).await.unwrap();
        let bytes = to_bytes(resp2.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["deliverables"][0]["status"], "rejected");
    }

    #[tokio::test]
    async fn accept_404_for_unknown_project() {
        use axum::routing::post as axum_post;
        let registry_root = tempfile::tempdir().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws_dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(ws_dir.path()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_project_registry(FileSystemProjectRegistry::new(registry_root.path()));
        let app = Router::new()
            .route(
                "/api/deliverables/{project}/{task}/accept",
                axum_post(accept_deliverable),
            )
            .with_state(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/deliverables/no-such/task-00000000-0000-0000-0000-000000000000/accept")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
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
