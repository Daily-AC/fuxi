//! 三个端点（Decision 22 phase 1 + phase 3 预览扩展）：
//! - `GET /api/deliverables` —— 列表
//! - `GET /api/deliverables/<project>/<task>/files/<name>` —— 下载
//! - `GET /api/deliverables/<project>/<task>/preview/<name>` —— 内联预览
//!
//! 数据源：扫 `project_registry` 列出的全部 project，每个 project 走
//! `<projects_root>/<project>/deliverables/` 下所有 task 的 manifest.json。
//!
//! v1 形态：扁平列表，按 `produced_at` 倒序（新的在前）。后续若量级上去
//! 加索引 / pagination。
//!
//! 503 路径：registry 未注入 → 返 unavailable，PWA 应跳过收件箱标签。
//!
//! ## 文件读取的两个端点
//!
//! - `/files/<name>` —— 设 `Content-Disposition: attachment`，浏览器**触发下载**。
//! - `/preview/<name>` —— 不设 attachment，**inline 返回**让浏览器/前端
//!   `<img>` `<pre>` 直接渲染。md 类用 `text/markdown` 让前端拉文本后自渲染。
//!
//! 两端点共享 `read_bucket_file()` 路径校验 + 读文件逻辑，仅响应头差异。
//! **不**用 `?inline=1` query 切换——分两条路径让 URL 自带语义，命中网关
//! 缓存策略时也方便区分（attachment 一般不缓存，inline 可短期缓存）。

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
/// 行为：
/// - 必发 DeliverableAccepted 事件（task + accepted_to 入 EventBus 审计）
/// - 若 `body.accepted_to` 非空且是合法绝对路径目录 → 把 deliverables bucket 里
///   每个 manifest 文件复制到 `<accepted_to>/<file.name>`（mkdir -p target；同名
///   覆盖）。
/// - accepted_to 为相对路径 / 系统敏感路径（`/etc` / `/sys` / `/proc` / `/usr`）
///   → 400 BadRequest 拒绝
/// - 文件 IO 失败（权限 / 磁盘满）→ 500 Internal，事件**已发**所以审计不丢
pub async fn accept_deliverable(
    State(state): State<AppState>,
    AxumPath((project_raw, task_raw)): AxumPath<(String, String)>,
    Json(body): Json<AcceptRequest>,
) -> Result<StatusCode> {
    let (project_id, task_id) = parse_project_and_task(&state, &project_raw, &task_raw).await?;

    // 校验 accepted_to——绝对路径 + 不在系统敏感目录
    if let Some(target) = &body.accepted_to {
        validate_accept_target(target)?;
    }

    // 先发事件——audit 优先；后续 copy 失败也保留事件痕迹
    let mut meta = EventMeta::now();
    meta.task = Some(task_id);
    let _ = state.fuxi.bus().publish(Event {
        meta,
        kind: EventKind::DeliverableAccepted {
            task: task_id,
            accepted_to: body.accepted_to.clone(),
        },
    });

    // accepted_to 提供 → 真实拷贝
    if let Some(target_dir) = body.accepted_to {
        let registry = state
            .project_registry
            .as_ref()
            .ok_or_else(|| Error::Internal("project_registry 突然丢了——竞态？".into()))?;
        copy_deliverable_files_to_target(registry, &project_id, task_id, &target_dir).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// 校验 accept 的 target_path：必须是绝对路径，不能在系统敏感目录。
///
/// 系统敏感目录列表保守：`/etc /sys /proc /usr /var /boot /bin /sbin /lib /lib64`。
/// `$HOME / /tmp / /Users / /Volumes` 等 user-friendly 路径全放行。fuxi-im 跑在
/// 用户 uid 下，OS 权限是兜底，但本端点先做应用层 sanity check 避免误操作。
///
/// **严格模式**（`FUXI_DELIVERABLE_STRICT=1`）：只允许写入 `$HOME` 子树。
/// 用户在生产部署时（systemd 跑 fuxi-im 暴露公网）开启它，限制 deliverable 接收
/// 范围让 web 端攻击者也只能往用户 home 里写——文件出 home 都拒。
/// 默认关闭，因为本机用户自用 `~/写作` 之类的 home 内路径不需要这个开关。
fn validate_accept_target(target: &std::path::Path) -> Result<()> {
    if !target.is_absolute() {
        return Err(Error::BadRequest(format!(
            "accepted_to 必须是绝对路径: {}",
            target.display()
        )));
    }
    let s = target.to_string_lossy();
    // macOS `tempfile::tempdir()` 返 `/var/folders/...`、Linux 也有 `/var/tmp` 是合法
    // 用户 tmp——所以不能 blanket reject `/var/`。下面列具体系统子目录。
    for prefix in [
        "/etc/",
        "/sys/",
        "/proc/",
        "/usr/",
        "/boot/",
        "/bin/",
        "/sbin/",
        "/lib/",
        "/lib64/",
        "/var/log/",
        "/var/lib/",
        "/var/run/",
        "/var/cache/",
        "/var/spool/",
    ] {
        if s.starts_with(prefix) {
            return Err(Error::BadRequest(format!(
                "accepted_to 不允许写入系统目录: {}",
                target.display()
            )));
        }
    }
    if deliverable_strict_mode_enabled() {
        let home = std::env::var("HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                Error::Internal("FUXI_DELIVERABLE_STRICT 开启但 $HOME 未设——无法做严格校验".into())
            })?;
        let canon_home = home.canonicalize().unwrap_or(home);
        // 用 canonicalize 是必须——target 可能是 `~/写作` 经 fuxi-im 解析后的
        // 软链或 .. 路径，不归一会被绕过。target 不存在时 canonicalize 失败，
        // 走父目录逐级试。
        let canon_target = target.canonicalize().unwrap_or_else(|_| {
            target
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .map(|p| p.join(target.file_name().unwrap_or_default()))
                .unwrap_or_else(|| target.to_path_buf())
        });
        if !canon_target.starts_with(&canon_home) {
            return Err(Error::BadRequest(format!(
                "FUXI_DELIVERABLE_STRICT 模式：accepted_to 必须在 $HOME ({}) 子树内，\
                 实际 {}",
                canon_home.display(),
                canon_target.display()
            )));
        }
    }
    Ok(())
}

/// 读 `FUXI_DELIVERABLE_STRICT` env，`1` / `true` / `yes` / `on`（不区分大小写）
/// 视为开启。其他全关闭——默认值即关闭，给本机自用场景免配置。
fn deliverable_strict_mode_enabled() -> bool {
    matches!(
        std::env::var("FUXI_DELIVERABLE_STRICT")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// 把 deliverable bucket 里每个 manifest 文件复制到 target_dir。
///
/// - mkdir -p target_dir（不存在则建）
/// - 每个文件源 = `<projects_root>/<project>/deliverables/<task>/<file.name>`
/// - 每个文件目标 = `<target_dir>/<file.name>`，存在则覆盖
/// - manifest.json 本身**不**复制（那是 fuxi 内部元信息，用户拿不上意义）
async fn copy_deliverable_files_to_target(
    registry: &fuxi_workspace::FileSystemProjectRegistry,
    project_id: &ProjectId,
    task: TaskId,
    target_dir: &std::path::Path,
) -> Result<()> {
    let bucket = registry
        .root()
        .join(project_id.as_str())
        .join("deliverables")
        .join(task.to_string());
    let manifest_path = bucket.join("manifest.json");
    if !manifest_path.exists() {
        return Err(Error::NotFound(format!(
            "task {task} 无 deliverables bucket"
        )));
    }
    let bytes = fs::read(&manifest_path)
        .await
        .map_err(|e| Error::Internal(format!("读 manifest 失败: {e}")))?;
    let manifest: fuxi_workspace::DeliverableManifest = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Internal(format!("解析 manifest 失败: {e}")))?;

    fs::create_dir_all(target_dir)
        .await
        .map_err(|e| Error::Internal(format!("mkdir target 失败: {e}")))?;

    // 收所有 file.name，去重（同 task 多次 produce 可能同名 → 取最新一份；
    // manifest entries 已按时间序，后写胜，HashMap 自然覆盖）
    let mut latest_files: HashMap<String, ()> = HashMap::new();
    for entry in &manifest.entries {
        for file in &entry.files {
            latest_files.insert(file.name.clone(), ());
        }
    }
    for name in latest_files.keys() {
        // 防 path traversal——name 不该含 `/` `\\` `..`，manifest 写时已校验，
        // 但 defense in depth 再过一遍
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(Error::BadRequest(format!("manifest 含非法文件名 {name:?}")));
        }
        let src = bucket.join(name);
        let dst = target_dir.join(name);
        fs::copy(&src, &dst).await.map_err(|e| {
            Error::Internal(format!(
                "copy {} → {} 失败: {e}",
                src.display(),
                dst.display()
            ))
        })?;
    }
    Ok(())
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
    let (bytes, ct) = read_bucket_file(&state, &project_raw, &task_raw, &name).await?;
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

/// `GET /api/deliverables/:project/:task/preview/:name` —— **inline** 返交付
/// 文件内容，给前端预览用（`<img src=...>` / `fetch().text()` → markdown 渲染 /
/// `<pre>` 文本显示）。
///
/// 与 `download_file` 共享路径 + 内容读取；唯一差异在响应头：
/// - **不**设 `Content-Disposition: attachment`，让浏览器 inline 处理
/// - 文本类（md/txt/csv/json）显式带 `charset=utf-8`，由 `guess_content_type` 处理
///
/// 安全：复用同一 bucket-bound 校验，无额外攻击面。
pub async fn preview_file(
    State(state): State<AppState>,
    AxumPath((project_raw, task_raw, name)): AxumPath<(String, String, String)>,
) -> std::result::Result<Response, Error> {
    let (bytes, ct) = read_bucket_file(&state, &project_raw, &task_raw, &name).await?;
    // inline disposition 显式声明——某些浏览器（旧 Safari）默认行为不一致，显式
    // `inline; filename=...` 让"另存为"对话框带文件名而不是变 attachment。
    let resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{name}\""),
        )
        .body(Body::from(bytes))
        .map_err(|e| Error::Internal(format!("构造 response 失败: {e}")))?;
    Ok(resp)
}

/// 共享 helper：路径校验 + 读 bucket 文件，返 `(bytes, content_type)`。
///
/// 校验链同原 `download_file`：project 注册 + task 段合法 + 文件名安全 + bucket 内。
/// 把这三步抽出来避免 `download_file` / `preview_file` 复制粘贴时漏掉某条防御。
async fn read_bucket_file(
    state: &AppState,
    project_raw: &str,
    task_raw: &str,
    name: &str,
) -> std::result::Result<(Vec<u8>, &'static str), Error> {
    let registry = state
        .project_registry
        .as_ref()
        .ok_or_else(|| Error::Unavailable("project_registry 未注入".into()))?;

    // 1. project 校验 + lookup
    let project_id = ProjectId::new(project_raw.to_string())
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
        .join(task_raw);
    let file_path = bucket.join(name);

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

    // 5. 读文件
    let mut file = fs::File::open(&canon_file)
        .await
        .map_err(|e| Error::Internal(format!("打开文件失败: {e}")))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .await
        .map_err(|e| Error::Internal(format!("读文件失败: {e}")))?;

    let ct = guess_content_type(name);
    Ok((bytes, ct))
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
            .route(
                "/api/deliverables/{project}/{task}/preview/{name}",
                axum_get(preview_file),
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

    /// bug #76 · preview 端点 = 同 download 内容但响应头是 `inline` 不是 `attachment`，
    /// 让浏览器内联渲染（md / 文本类）/ 直显（图片）。前端 DeliverableDetailPage
    /// FilePreview 走这个 URL 拉文本 + 触 Markdown 组件。
    #[tokio::test]
    async fn preview_returns_inline_content_disposition() {
        let (_root, app, _r, project, task) = make_app_with_data().await;
        let req = Request::builder()
            .uri(format!(
                "/api/deliverables/{project}/{task}/preview/report.md"
            ))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cd = resp
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        assert!(
            cd.starts_with("inline"),
            "preview 端点必须设 inline disposition，实际：{cd}"
        );
        assert!(
            !cd.contains("attachment"),
            "preview 端点不应含 attachment（那是 /files/ 的事）"
        );
        let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let s = String::from_utf8(body.to_vec()).unwrap();
        assert!(s.contains("# 报告"));
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
    async fn accept_with_target_path_copies_files() {
        use axum::routing::post as axum_post;

        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_repo_td, repo) = make_repo().await;
        let project_obj = registry.add(repo, Some("erp".into()), None).await.unwrap();

        // 落两条文件 deliverable
        let task = TaskId::new();
        let mgr = DeliverablesManager::new(project_obj.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        let f1 = src_dir.path().join("report.md");
        let f2 = src_dir.path().join("data.csv");
        tokio::fs::write(&f1, "# 报告内容").await.unwrap();
        tokio::fs::write(&f2, "a,b\n1,2").await.unwrap();
        mgr.produce(task, DeliverableKind::ResearchSummary, &[f1, f2])
            .await
            .unwrap();

        // 起 router
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

        // 用户指定 accepted_to 在 tempdir（绝对路径，非系统敏感）
        let target_dir = tempfile::tempdir().unwrap();
        let target_path = target_dir.path().join("写作");

        let body = serde_json::json!({
            "accepted_to": target_path.to_string_lossy(),
        });
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/deliverables/erp/task-{}/accept", task.0))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // target dir 下应有两份文件，内容同源
        assert!(
            target_path.join("report.md").is_file(),
            "report.md 应被复制"
        );
        assert!(target_path.join("data.csv").is_file(), "data.csv 应被复制");
        assert_eq!(
            tokio::fs::read_to_string(target_path.join("report.md"))
                .await
                .unwrap(),
            "# 报告内容"
        );
    }

    #[tokio::test]
    async fn accept_rejects_relative_target_path() {
        use axum::routing::post as axum_post;

        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_repo_td, repo) = make_repo().await;
        let project_obj = registry.add(repo, Some("erp".into()), None).await.unwrap();
        let task = TaskId::new();
        let mgr = DeliverablesManager::new(project_obj.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        let f = src_dir.path().join("x.md");
        tokio::fs::write(&f, "x").await.unwrap();
        mgr.produce(task, DeliverableKind::ResearchSummary, &[f])
            .await
            .unwrap();

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

        // 相对路径 → 400
        let body = serde_json::json!({"accepted_to": "relative/path"});
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/deliverables/erp/task-{}/accept", task.0))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn accept_rejects_system_path() {
        use axum::routing::post as axum_post;

        let registry_root = tempfile::tempdir().unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let (_repo_td, repo) = make_repo().await;
        let project_obj = registry.add(repo, Some("erp".into()), None).await.unwrap();
        let task = TaskId::new();
        let mgr = DeliverablesManager::new(project_obj.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        let f = src_dir.path().join("x.md");
        tokio::fs::write(&f, "x").await.unwrap();
        mgr.produce(task, DeliverableKind::ResearchSummary, &[f])
            .await
            .unwrap();

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

        // /etc 系统路径 → 400
        let body = serde_json::json!({"accepted_to": "/etc/passwd"});
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/deliverables/erp/task-{}/accept", task.0))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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

    /// Decision 22 phase 3 strict mode：覆盖 4 个场景（默认关 / 开 + 内 /
    /// 开 + 外 / 开 + 系统目录 / 开 + $HOME 缺）。
    ///
    /// **合并成一个测试函数**——env 是进程全局，cargo test 默认并发跑同 binary
    /// 测试会撞 set_var/remove_var 串扰。所以本测试不依赖 #[serial] crate，
    /// 把所有断言塞一个测试里串行跑，外面 strict 模式始终关掉以隔离其他测试。
    #[test]
    fn validate_accept_target_strict_mode_covers_all_cases() {
        // 备份 + finally 还原 env，避免污染其他测试（HOME 一般是真实存在的）。
        let saved_strict = std::env::var("FUXI_DELIVERABLE_STRICT").ok();
        let saved_home = std::env::var("HOME").ok();
        let restore = |strict: Option<String>, home: Option<String>| unsafe {
            match strict {
                Some(v) => std::env::set_var("FUXI_DELIVERABLE_STRICT", v),
                None => std::env::remove_var("FUXI_DELIVERABLE_STRICT"),
            }
            match home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        };

        // 1) 默认关闭：/tmp/anywhere 通过
        unsafe {
            std::env::remove_var("FUXI_DELIVERABLE_STRICT");
        }
        assert!(
            validate_accept_target(std::path::Path::new("/tmp/anywhere")).is_ok(),
            "默认关闭：/tmp/anywhere 应通过"
        );

        // 2) 开启 + $HOME 内：通过
        let home_dir = tempfile::tempdir().expect("tmp home");
        let home_str = home_dir.path().to_string_lossy().into_owned();
        let inside = home_dir.path().join("写作");
        unsafe {
            std::env::set_var("FUXI_DELIVERABLE_STRICT", "1");
            std::env::set_var("HOME", &home_str);
        }
        assert!(validate_accept_target(&inside).is_ok(), "$HOME/写作 应通过");

        // 3) 开启 + 非 $HOME：拒
        let outside = std::path::PathBuf::from("/tmp/elsewhere");
        match validate_accept_target(&outside) {
            Err(Error::BadRequest(msg)) => {
                assert!(msg.contains("FUXI_DELIVERABLE_STRICT"), "msg: {msg}");
            }
            other => {
                restore(saved_strict.clone(), saved_home.clone());
                panic!("/tmp/elsewhere 应被拒，实际 {other:?}");
            }
        }

        // 4) 开启 + 系统目录：仍走系统黑名单（先于 strict 拒）
        let r = validate_accept_target(std::path::Path::new("/etc/foo"));
        assert!(matches!(r, Err(Error::BadRequest(_))), "got {r:?}");

        // 5) 开启 + $HOME 缺：Internal 500
        unsafe {
            std::env::remove_var("HOME");
        }
        let r = validate_accept_target(std::path::Path::new("/anywhere"));
        assert!(matches!(r, Err(Error::Internal(_))), "got {r:?}");

        // 还原 env，避免污染其他测试。
        restore(saved_strict, saved_home);
    }
}
