//! `POST /api/upload` + `GET /api/uploads/:id` —— 文件上传 / 下载（Task #17）。
//!
//! 协议：
//! - **上传**：multipart/form-data，field 名 `file`。服务端读字节、白名单 mime
//!   校验、sha256 hash、落 `~/.fuxi/im_uploads/<sha[:2]>/<sha>.<ext>` + 写库
//! - **下载**：用 id 查 uploads 表 → 流式读盘 → 带 Content-Type +
//!   Content-Disposition (inline；filename 来自 record.name)
//!
//! 鉴权：cookie auth layer 已覆盖 /api/* 全网，本 handler 不重复检查。

use crate::error::{Error, Result};
use crate::state::AppState;
use crate::uploads::UploadRecord;
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub id: String,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub bytes: i64,
    pub sha256: String,
}

impl From<UploadRecord> for UploadResponse {
    fn from(r: UploadRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            mime: r.mime,
            bytes: r.bytes,
            sha256: r.sha256,
        }
    }
}

pub async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<UploadResponse>> {
    let store = state
        .upload_store
        .as_ref()
        .ok_or_else(|| Error::Unavailable("upload_store 未注入".into()))?;

    // 找 field 名 = "file" 的那一份；其它 field 全跳。
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| Error::BadRequest(format!("multipart 读取失败：{e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        if field_name != "file" {
            continue;
        }
        let mime = field.content_type().map(|s| s.to_string());
        let name = field.file_name().map(|s| s.to_string());
        let bytes = field
            .bytes()
            .await
            .map_err(|e| Error::BadRequest(format!("读 multipart 字节失败：{e}")))?;
        let rec = store
            .put(&bytes, name.as_deref(), mime.as_deref(), None)
            .await?;
        return Ok(Json(rec.into()));
    }
    Err(Error::BadRequest("multipart 缺 field=file".into()))
}

pub async fn get_upload(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response> {
    let store = state
        .upload_store
        .as_ref()
        .ok_or_else(|| Error::Unavailable("upload_store 未注入".into()))?;
    let rec = store
        .get(&id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("upload {id} 不存在")))?;
    let bytes = store.read_bytes(&rec)?;

    let mut headers = HeaderMap::new();
    if let Some(mime) = rec.mime.as_deref()
        && let Ok(v) = HeaderValue::from_str(mime)
    {
        headers.insert(header::CONTENT_TYPE, v);
    }
    let disposition = match rec.name.as_deref() {
        Some(n) => format!("inline; filename=\"{}\"", sanitize_filename(n)),
        None => "inline".to_string(),
    };
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    let mut resp = (StatusCode::OK, Body::from(bytes)).into_response();
    resp.headers_mut().extend(headers);
    Ok(resp)
}

/// Content-Disposition filename 里出现 `"` `\\` 会破坏 header；简单转义。
/// 真要支持 Unicode 文件名应该走 RFC 5987 `filename*=utf-8''<percent-encoded>`，
/// 但 PWA 主要场景是图片/PDF，名字里不太会带这些字符；保守转义即可。
fn sanitize_filename(name: &str) -> String {
    name.replace(['"', '\\'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use crate::uploads::UploadStore;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::{get, post};
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
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
        let out = tokio::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    async fn build_app() -> (tempfile::TempDir, Router) {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(&dir.path().join("im.db")).await.expect("init");
        let upload_store = UploadStore::new(pool, dir.path().join("uploads"));

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_upload_store(upload_store);
        let app = Router::new()
            .route("/api/upload", post(super::upload))
            .route("/api/uploads/{id}", get(super::get_upload))
            .with_state(state);
        (dir, app)
    }

    /// 构造一个带 boundary 的 multipart body：单 field=file。
    fn multipart_body(boundary: &str, filename: &str, content_type: &str, bytes: &[u8]) -> Vec<u8> {
        let head = format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: {content_type}\r\n\r\n"
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let mut buf = Vec::with_capacity(head.len() + bytes.len() + tail.len());
        buf.extend_from_slice(head.as_bytes());
        buf.extend_from_slice(bytes);
        buf.extend_from_slice(tail.as_bytes());
        buf
    }

    #[tokio::test]
    async fn upload_then_download_roundtrip() {
        let (_dir, app) = build_app().await;
        let boundary = "----b1";
        let body = multipart_body(boundary, "hi.txt", "text/plain", b"hello");

        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = parsed["id"].as_str().expect("id").to_string();
        assert_eq!(parsed["bytes"], 5);
        assert_eq!(parsed["mime"], "text/plain");
        assert_eq!(parsed["name"], "hi.txt");

        // 下载回来
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("/api/uploads/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(ct.starts_with("text/plain"), "Content-Type: {ct}");
        let cd = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(cd.contains("inline"));
        assert!(cd.contains("hi.txt"), "Content-Disposition: {cd}");
        let body_bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        assert_eq!(&body_bytes[..], b"hello");
    }

    #[tokio::test]
    async fn upload_rejects_bad_mime() {
        let (_dir, app) = build_app().await;
        let boundary = "----b2";
        let body = multipart_body(boundary, "evil.sh", "application/x-shellscript", b"x");
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), 4 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            parsed["message"].as_str().unwrap_or("").contains("白名单"),
            "msg: {parsed}"
        );
    }

    #[tokio::test]
    async fn upload_rejects_missing_file_field() {
        let (_dir, app) = build_app().await;
        let boundary = "----b3";
        let body = format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"other\"\r\n\r\nx\r\n--{boundary}--\r\n"
        );
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_upload_unknown_returns_404() {
        let (_dir, app) = build_app().await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/uploads/not-existing")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn upload_503_when_store_not_injected() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi); // 默认不注 upload_store
        let app = Router::new()
            .route("/api/upload", post(super::upload))
            .with_state(state);

        let boundary = "----b4";
        let body = multipart_body(boundary, "x.txt", "text/plain", b"x");
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/upload")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
