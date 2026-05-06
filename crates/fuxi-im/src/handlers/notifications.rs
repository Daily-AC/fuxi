//! `/api/notifications` —— PWA「通知」tab 数据源。
//!
//! 路由：
//! - `GET /api/notifications` —— 列表 + unread_count（前端 badge 用）
//! - `POST /api/notifications/{id}/read` —— 标已读（不删，仅清红点）
//! - `POST /api/notifications/{id}/close` —— 关闭（默认列表隐藏）
//! - `POST /api/notifications/read-all` —— 一键全部标已读（红点清零）
//!
//! 通知来源：
//! - bug：玄女自己 `fuxi bug report` 落档（fuxi-cli 直开 SQLite 写）
//! - review_request：门客交付时 emit（后续 task #8 接 deliverables）
//! - context_handoff_offer：玄女 context 达 45% 时 fuxi 主动 emit（task #8）
//! - system：平台级提示（暂无具体来源）

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::notifications::{ListFilter, Notification};
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationsResponse {
    pub notifications: Vec<Notification>,
    pub unread_count: i64,
}

/// `GET /api/notifications` —— 列未关闭的通知 + 未读计数。
///
/// query 参数（可选）：
/// - `kind`：过滤 kind（"bug" / "review_request" / ...）
/// - `include_closed`：包含已关闭（默认 false）
/// - `limit`：上限（默认 200）
pub async fn list(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Result<Json<NotificationsResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    let filter = ListFilter {
        kind: q.kind,
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

pub async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ActionResponse>> {
    let store = state
        .notifications
        .as_ref()
        .ok_or_else(|| Error::Unavailable("通知存储未注入".into()))?;
    store.close(&id).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::notifications::{NewNotification, NotificationStore};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
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
    }

    #[tokio::test]
    async fn close_endpoint_hides_from_list() {
        let (_dir, app, store) = build_app().await;
        let n = store.insert(NewNotification::bug("a", "x")).await.unwrap();

        let resp = app
            .clone()
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
        assert!(body.notifications.is_empty());
        assert_eq!(body.unread_count, 0);
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
