//! Phase 1 · `/api/topics` —— 话题 CRUD + 切换 HTTP 端点。
//!
//! PWA 桌面 left sidebar / 移动端左滑抽屉的数据源。
//!
//! 端点：
//! - `GET    /api/topics?include_archived=0|1`  列 topic
//! - `POST   /api/topics`                       建 topic（body `{title}`）
//! - `POST   /api/topics/:id/switch`            切玄女当前 topic（常驻分身秒切，Phase 2）
//! - `POST   /api/topics/:id/archive`           归档
//! - `POST   /api/topics/:id/read`              推进未读水位（PWA 进/离话题时调）
//! - `GET    /api/topics/current`               读当前 topic（PWA 启动时拉，避免轮询）
//!
//! 鉴权走全局 cookie_auth_layer（已 wire 在 router.rs）。topic_store / fuxi 未注入
//! 时返 503——production `fuxi im start` 必注入 topic_store。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use fuxi_core::{TopicId, TopicMeta};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `/api/topics` 单条响应 wire 形态。跟 `TopicMeta` 字段平铺；前端按 `id` 字符串
/// 比较 current_topic_id。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicView {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
    /// Phase 2 · 未读数（水位之后的非 user 消息）。current topic 恒 0。
    #[serde(default)]
    pub unread_count: i64,
}

impl From<TopicMeta> for TopicView {
    fn from(m: TopicMeta) -> Self {
        Self {
            id: m.id.0.to_string(),
            title: m.title,
            created_at: m.created_at,
            last_active_at: m.last_active_at,
            pinned: m.pinned,
            archived_at: m.archived_at,
            unread_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicsResponse {
    pub topics: Vec<TopicView>,
    /// 当前玄女绑定的 topic_id（uuid 字符串）。前端高亮 sidebar 行用。
    pub current_topic_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentTopicResponse {
    pub current_topic_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    /// `1` / `true` 一并显示归档；缺省只列活跃。
    #[serde(default)]
    pub include_archived: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTopicRequest {
    pub title: String,
}

/// `GET /api/topics` —— 列 topic + 当前 topic_id。
pub async fn list_topics(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<TopicsResponse>> {
    let store = state
        .topic_store
        .as_ref()
        .ok_or_else(|| Error::Unavailable("topic_store 未注入".into()))?;
    let include_archived = parse_bool(q.include_archived.as_deref());
    let items = store
        .list(include_archived)
        .await
        .map_err(|e| Error::Internal(format!("topic list: {e}")))?;
    let unread = store.unread_counts().await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "unread_counts 失败——badge 退化为 0，不挡列表");
        Default::default()
    });
    let current_topic_id = state.fuxi.current_topic_id().0.to_string();
    let topics = items
        .into_iter()
        .map(|m| {
            let mut v = TopicView::from(m);
            // current topic 用户正看着（WS 实时在喂）→ 恒 0，不显小红点。
            if v.id != current_topic_id {
                v.unread_count = unread.get(&v.id).copied().unwrap_or(0);
            }
            v
        })
        .collect();
    Ok(Json(TopicsResponse {
        topics,
        current_topic_id,
    }))
}

/// `POST /api/topics` —— 建 topic。返回 201 + 新 TopicView。
pub async fn create_topic(
    State(state): State<AppState>,
    Json(req): Json<CreateTopicRequest>,
) -> Result<(StatusCode, Json<TopicView>)> {
    let store = state
        .topic_store
        .as_ref()
        .ok_or_else(|| Error::Unavailable("topic_store 未注入".into()))?;
    let title = req.title.trim();
    if title.is_empty() {
        return Err(Error::BadRequest("title 不能为空".into()));
    }
    if title.chars().count() > 80 {
        return Err(Error::BadRequest("title 上限 80 字".into()));
    }
    let m = store
        .create(title)
        .await
        .map_err(|e| Error::Internal(format!("topic create: {e}")))?;
    Ok((StatusCode::CREATED, Json(TopicView::from(m))))
}

/// `POST /api/topics/:id/switch` —— 切玄女当前 topic。
/// 走 `XuannvSwitcher` trait（fuxi-cli `topic_switch::switch_topic_to` 注入实现），
/// 避免 fuxi-im 直接依赖 fuxi-cli（循环依赖）。
/// Phase 2：ensure 常驻分身 + set_current_topic，热路径毫秒级（池命中）；
/// 懒启动冷路径数秒（spawn 分身 + drain 持久队列），axum handler 等到完成才返回。
pub async fn switch_topic(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<CurrentTopicResponse>> {
    let topic_id = parse_topic_id(&id)?;
    let switcher = state.fuxi.xuannv_switcher().await.ok_or_else(|| {
        Error::Unavailable("xuannv_switcher 未注入（fuxi im start 未配置？）".into())
    })?;
    switcher
        .switch_topic(topic_id)
        .await
        .map_err(|e| Error::Internal(format!("switch_topic: {e}")))?;
    Ok(Json(CurrentTopicResponse {
        current_topic_id: state.fuxi.current_topic_id().0.to_string(),
    }))
}

/// `POST /api/topics/:id/archive` —— 归档。
pub async fn archive_topic(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode> {
    let topic_id = parse_topic_id(&id)?;
    let store = state
        .topic_store
        .as_ref()
        .ok_or_else(|| Error::Unavailable("topic_store 未注入".into()))?;
    store
        .archive(topic_id)
        .await
        .map_err(|e| Error::Internal(format!("topic archive: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/topics/:id/read` —— 推进未读水位（PWA 进入/离开话题时调）。
pub async fn mark_topic_read(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode> {
    let topic_id = parse_topic_id(&id)?;
    let store = state
        .topic_store
        .as_ref()
        .ok_or_else(|| Error::Unavailable("topic_store 未注入".into()))?;
    store
        .mark_read(topic_id)
        .await
        .map_err(|e| Error::Internal(format!("topic mark_read: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/topics/current` —— 单独返当前 topic_id（PWA 高频轮询/对账兜底）。
pub async fn current_topic(State(state): State<AppState>) -> Json<CurrentTopicResponse> {
    Json(CurrentTopicResponse {
        current_topic_id: state.fuxi.current_topic_id().0.to_string(),
    })
}

fn parse_topic_id(s: &str) -> Result<TopicId> {
    let uuid = Uuid::parse_str(s.trim())
        .map_err(|e| Error::BadRequest(format!("topic_id 非 UUID: {e}")))?;
    Ok(TopicId::from(uuid))
}

fn parse_bool(s: Option<&str>) -> bool {
    matches!(
        s.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::topic_store::TopicStore;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn build_router() -> (tempfile::TempDir, Router) {
        let (dir, app, _store, _conv) = build_router_with_store().await;
        (dir, app)
    }

    /// build_router 的扩展版：TopicStore + 同 pool 的 ConvStore 一并返出来，
    /// 未读水位测试要直接灌消息库（TopicStore.pool 是私有字段，跨模块拿不到）。
    async fn build_router_with_store() -> (
        tempfile::TempDir,
        Router,
        TopicStore,
        crate::conv_store::ConvStore,
    ) {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = db::init_at(&dir.path().join("im.db")).await.expect("init");
        let store = TopicStore::new(pool.clone());
        let conv = crate::conv_store::ConvStore::new(pool);
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(dir.path()));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_topic_store(store.clone());
        let app = Router::new()
            .route("/api/topics", get(list_topics).post(create_topic))
            .route("/api/topics/current", get(current_topic))
            .route("/api/topics/{id}/archive", post(archive_topic))
            .route("/api/topics/{id}/read", post(mark_topic_read))
            .with_state(state);
        (dir, app, store, conv)
    }

    /// 往 topic 灌一条 xuannv 消息——未读计数口径用（同 topic_store.rs C1 的 seed）。
    async fn seed_xuannv_msg(conv: &crate::conv_store::ConvStore, topic: TopicId) {
        let conv_id = conv
            .ensure_scope(crate::conv_store::SCOPE_XUANNV, None)
            .await
            .unwrap();
        conv.append_message_in_topic(
            &conv_id,
            "xuannv",
            None,
            "text",
            &serde_json::json!({"text": "完工汇报"}),
            None,
            None,
            chrono::Utc::now(),
            &topic.0.to_string(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_topics_carries_unread_and_forces_current_zero() {
        // current topic 强制 0（用户正看着，WS 实时在喂）；其他 topic 按水位计数。
        let (_dir, app, store, conv) = build_router_with_store().await;
        let m = store.create("别的话题").await.unwrap();
        store.mark_read(m.id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        seed_xuannv_msg(&conv, m.id).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/topics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: TopicsResponse = serde_json::from_slice(&bytes).unwrap();
        let view = parsed
            .topics
            .iter()
            .find(|t| t.id == m.id.0.to_string())
            .unwrap();
        assert_eq!(view.unread_count, 1, "非 current topic 按水位计未读");
        let general = parsed
            .topics
            .iter()
            .find(|t| t.id == TopicId::general().0.to_string())
            .unwrap();
        assert_eq!(general.unread_count, 0, "current topic 强制 0");
    }

    #[tokio::test]
    async fn mark_topic_read_endpoint_returns_204_and_clears() {
        let (_dir, app, store, conv) = build_router_with_store().await;
        let m = store.create("读我").await.unwrap();
        store.mark_read(m.id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        seed_xuannv_msg(&conv, m.id).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/topics/{}/read", m.id.0))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let counts = store.unread_counts().await.unwrap();
        assert_eq!(counts.get(&m.id.0.to_string()), None, "read 后未读清零");
    }

    #[tokio::test]
    async fn list_returns_general_seed_plus_current() {
        let (_dir, app) = build_router().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/topics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: TopicsResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(
            parsed.topics.iter().any(|t| t.title == "general"),
            "general 兜底 topic 必在 list"
        );
        // 初始 current 是 general
        assert_eq!(parsed.current_topic_id, TopicId::general().0.to_string());
    }

    #[tokio::test]
    async fn create_then_list_includes_new_topic() {
        let (_dir, app) = build_router().await;
        let body = serde_json::to_vec(&serde_json::json!({"title": "周末画画"})).unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/topics")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let new_topic: TopicView = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(new_topic.title, "周末画画");

        // 列时新 topic 应在
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/topics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: TopicsResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(
            parsed.topics.iter().any(|t| t.id == new_topic.id),
            "新建的 topic 应出现在 list"
        );
    }

    #[tokio::test]
    async fn create_rejects_empty_title() {
        let (_dir, app) = build_router().await;
        let body = serde_json::to_vec(&serde_json::json!({"title": "  "})).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/topics")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn archive_then_list_default_excludes_it() {
        let (_dir, app) = build_router().await;
        let body = serde_json::to_vec(&serde_json::json!({"title": "to_archive"})).unwrap();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/topics")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let new_topic: TopicView =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = new_topic.id.clone();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/topics/{id}/archive"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/topics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let parsed: TopicsResponse =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(
            parsed.topics.iter().all(|t| t.id != id),
            "归档 topic 不应出现在默认 list"
        );

        // include_archived=1 时应回来
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/topics?include_archived=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let parsed: TopicsResponse =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(
            parsed.topics.iter().any(|t| t.id == id),
            "include_archived=1 时归档应回来"
        );
    }

    #[tokio::test]
    async fn current_topic_endpoint_returns_general_at_boot() {
        let (_dir, app) = build_router().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/topics/current")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: CurrentTopicResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.current_topic_id, TopicId::general().0.to_string());
    }
}
