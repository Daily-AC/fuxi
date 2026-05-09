//! `/api/notifications` —— PWA「通知」tab 数据源 + issue 工作流入口。
//!
//! 路由：
//! - `GET    /api/notifications` —— 列表 + unread_count（前端 badge 用）。
//!   query: `kind` / `status` / `include_closed` / `limit`
//! - `POST   /api/notifications/{id}/read` —— 标已读（不删，仅清红点）
//! - `POST   /api/notifications/{id}/close` —— 关闭（status='closed'）
//! - `POST   /api/notifications/{id}/reopen` —— 重开（status='open'）
//! - `POST   /api/notifications/{id}/link-fix` —— Claude 关联 fix commit
//! - `PATCH  /api/notifications/{id}` —— 通用状态机转换（{status, note?}）
//! - `POST   /api/notifications/read-all` —— 一键全部标已读（红点清零）
//!
//! actor 推断：cookie auth → "user"；玄女 device token → "xuannv"；CLI（绕过
//! HTTP 直开 SQLite）→ "claude"。当前 handler 简单按"是否带 cookie"判定，
//! 玄女 / Claude 走 device token + 在 body 显式带 actor。
//!
//! 通知来源：
//! - bug：玄女自己 `fuxi bug report` 落档（fuxi-cli 直开 SQLite 写）
//! - review_request：门客交付时 emit
//! - context_handoff_offer：玄女 context 达 45% 时 fuxi 主动 emit
//! - system：平台级提示

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::notifications::{FixRef, IssueStatus, ListFilter, Notification, now_iso};
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationsResponse {
    pub notifications: Vec<Notification>,
    pub unread_count: i64,
}

/// `GET /api/notifications` —— 列通知 + 未读计数。
///
/// query 参数（可选）：
/// - `kind`：过滤 kind（"bug" / "review_request" / ...）
/// - `status`：过滤 status（"open" / "awaiting_test" / "closed"）
/// - `include_closed`：包含已关闭（默认 false；指定 status 时该选项被忽略）
/// - `limit`：上限（默认 200）
pub async fn list(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Result<Json<NotificationsResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    let status = q.status.as_deref().map(parse_status).transpose()?;
    let filter = ListFilter {
        kind: q.kind,
        status,
        include_closed: q.include_closed.unwrap_or(false),
        limit: q.limit,
    };
    let notifications = store.list(filter).await?;
    let unread_count = store.unread_count().await?;
    Ok(Json(NotificationsResponse {
        notifications,
        unread_count,
    }))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub include_closed: Option<bool>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionResponse {
    pub ok: bool,
}

pub async fn mark_read(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    store.mark_read(&id).await?;
    Ok(Json(ActionResponse { ok: true }))
}

#[derive(Debug, Deserialize, Default)]
pub struct CloseRequest {
    /// 谁关的——前端 cookie 路径默认 "user"；玄女 device token 自填 "xuannv"
    #[serde(default)]
    pub actor: Option<String>,
    /// 备注理由，写进 events.note。可空。
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CloseRequest>>,
) -> Result<Json<ActionResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    let req = body.map(|Json(r)| r).unwrap_or_default();
    let actor = req.actor.as_deref().unwrap_or("user");
    store.close(&id, actor, req.note.as_deref()).await?;
    Ok(Json(ActionResponse { ok: true }))
}

pub async fn reopen(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CloseRequest>>,
) -> Result<Json<ActionResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    let req = body.map(|Json(r)| r).unwrap_or_default();
    let actor = req.actor.as_deref().unwrap_or("user");
    store.reopen(&id, actor, req.note.as_deref()).await?;
    Ok(Json(ActionResponse { ok: true }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateStatusRequest {
    pub status: String,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn update_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateStatusRequest>,
) -> Result<Json<ActionResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    let new_status = parse_status(&req.status)?;
    let actor = req.actor.as_deref().unwrap_or("user");
    store
        .update_status(&id, new_status, actor, req.note.as_deref())
        .await?;
    Ok(Json(ActionResponse { ok: true }))
}

#[derive(Debug, Deserialize)]
pub struct LinkFixRequest {
    pub commit_sha: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

pub async fn link_fix(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LinkFixRequest>,
) -> Result<Json<ActionResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    if req.commit_sha.trim().is_empty() {
        return Err(Error::BadRequest("commit_sha 不能为空".into()));
    }
    let fix = FixRef {
        commit_sha: req.commit_sha,
        branch: req.branch,
        summary: req.summary,
        at: now_iso(),
    };
    let actor = req.actor.as_deref().unwrap_or("claude");
    store.link_fix(&id, fix, actor).await?;
    Ok(Json(ActionResponse { ok: true }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadAllResponse {
    pub ok: bool,
    pub updated: i64,
}

pub async fn read_all(State(state): State<AppState>) -> Result<Json<ReadAllResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    let updated = store.mark_all_read().await?;
    Ok(Json(ReadAllResponse { ok: true, updated }))
}

fn parse_status(s: &str) -> Result<IssueStatus> {
    match s {
        "open" => Ok(IssueStatus::Open),
        "awaiting_test" => Ok(IssueStatus::AwaitingTest),
        "closed" => Ok(IssueStatus::Closed),
        other => Err(Error::BadRequest(format!(
            "status 必须是 open / awaiting_test / closed；收到 {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::notifications::{NewNotification, NotificationStore};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, patch, post};
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    async fn build_app() -> (TempDir, Router, NotificationStore) {
        let dir = tempfile::tempdir().expect("tmp");
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let pool = db::init_at(dir.path().join("im.db")).await.expect("init");
        let store = NotificationStore::new(pool);
        let state = AppState::new(fuxi).with_notifications(store.clone());
        let app = Router::new()
            .route("/api/notifications", get(list))
            .route("/api/notifications/read-all", post(read_all))
            .route("/api/notifications/{id}/read", post(mark_read))
            .route("/api/notifications/{id}/close", post(close))
            .route("/api/notifications/{id}/reopen", post(reopen))
            .route("/api/notifications/{id}/link-fix", post(link_fix))
            .route("/api/notifications/{id}", patch(update_status))
            .with_state(state);
        (dir, app, store)
    }

    #[tokio::test]
    async fn list_returns_open_with_unread_count() {
        let (_dir, app, store) = build_app().await;
        store.insert(NewNotification::bug("a", "x")).await.unwrap();
        store.insert(NewNotification::bug("b", "y")).await.unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/notifications")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: NotificationsResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.notifications.len(), 2);
        assert_eq!(body.unread_count, 2);
        // 新字段在 wire 上：
        for n in &body.notifications {
            assert_eq!(n.status, "open");
            assert!(n.fix_refs.is_empty());
            assert_eq!(n.events.len(), 1);
        }
    }

    #[tokio::test]
    async fn close_endpoint_with_actor_note_records_event() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();

        let req_body = serde_json::json!({"actor":"xuannv","note":"重复了"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/notifications/{}/close", n.id))
                    .header("content-type", "application/json")
                    .body(Body::from(req_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "closed");
        let last = after.events.last().unwrap();
        assert_eq!(last.actor, "xuannv");
        assert_eq!(last.note.as_deref(), Some("重复了"));
    }

    #[tokio::test]
    async fn close_endpoint_no_body_defaults_actor_user() {
        // PWA 老前端 close 不带 body，应仍能工作（actor 默认 "user"）
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/notifications/{}/close", n.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "closed");
        assert_eq!(after.events.last().unwrap().actor, "user");
    }

    #[tokio::test]
    async fn link_fix_endpoint_appends_ref_and_transitions() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();
        let body = serde_json::json!({
            "commit_sha": "63d3df0",
            "branch": "fix/foo",
            "summary": "修了"
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/notifications/{}/link-fix", n.id))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "awaiting_test");
        assert_eq!(after.fix_refs.len(), 1);
        assert_eq!(after.fix_refs[0].commit_sha, "63d3df0");
    }

    #[tokio::test]
    async fn link_fix_rejects_empty_sha() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();
        let body = serde_json::json!({"commit_sha": "  "});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/notifications/{}/link-fix", n.id))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn patch_status_transitions_to_awaiting_test() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();
        let body = serde_json::json!({"status":"awaiting_test","actor":"claude"});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/notifications/{}", n.id))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "awaiting_test");
    }

    #[tokio::test]
    async fn patch_status_invalid_returns_400() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();
        let body = serde_json::json!({"status":"banana"});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/notifications/{}", n.id))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_filter_by_status_query() {
        let (_dir, app, store) = build_app().await;
        let a = store.insert(NewNotification::bug("a", "")).await.unwrap();
        let _b = store.insert(NewNotification::bug("b", "")).await.unwrap();
        let _c = store.insert(NewNotification::bug("c", "")).await.unwrap();
        store.close(&a.id, "user", None).await.unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/notifications?status=closed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: NotificationsResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.notifications.len(), 1);
        assert_eq!(body.notifications[0].title, "a");
    }

    #[tokio::test]
    async fn reopen_endpoint_clears_closed_at() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "")).await.unwrap();
        store.close(&n.id, "user", None).await.unwrap();
        let body = serde_json::json!({"actor":"user","note":"还有问题"});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/notifications/{}/reopen", n.id))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "open");
        assert!(after.closed_at.is_none());
    }

    #[tokio::test]
    async fn read_all_zeros_unread_count() {
        let (_dir, app, store) = build_app().await;
        store.insert(NewNotification::bug("a", "x")).await.unwrap();
        store.insert(NewNotification::bug("b", "y")).await.unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/notifications/read-all")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let body: ReadAllResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.updated, 2);

        let resp2 = app
            .oneshot(
                Request::builder()
                    .uri("/api/notifications")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp2.into_body(), 1024 * 64).await.unwrap();
        let body: NotificationsResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.unread_count, 0);
    }

    #[tokio::test]
    async fn handler_returns_503_when_store_not_injected() {
        let dir = tempfile::tempdir().expect("tmp");
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi);
        let app = Router::new()
            .route("/api/notifications", get(list))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/notifications")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
