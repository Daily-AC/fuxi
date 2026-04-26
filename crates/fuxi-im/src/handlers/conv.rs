//! `/api/conv` + `/api/tasks/:id/stream` —— WebSocket 事件流。
//!
//! `/api/conv`（玄女对话流）：
//!   - 仅推 `meta.agent == xuannv_id` 的事件——所有跟玄女对话相关的 EventKind
//!     （`UserPrompted` / `AgentResponded` / `OrchestratorCcReceived` /
//!     `UserInterventionSent` / `AgentInterrupted` / `ThinkingStarted/Finished` /
//!     `ToolCallStarted/Finished`）都会带 `meta.agent = xuannv` 或抄送时 set 为玄女。
//!     直接按 agent id 过滤，**不维护一份白名单**——哪天加新 EventKind，只要 publisher
//!     正确 set `meta.agent` 就自动出现在玄女流里，不需要改 IM 代码。
//!
//! `/api/tasks/:id/stream`（单任务流）：
//!   - 仅推 `meta.task == Some(:id)` 的事件——任务级实时观察。
//!
//! 两者都接受 `?from=<event_id|rfc3339>` 做历史 + live tail；客户端断线自带 cursor
//! 重连——服务端不维护 session（这是 firehose 同款契约）。

use crate::conv_store::{Message, SCOPE_XUANNV};
use crate::error::{Error, Result};
use crate::handlers::ws_common::{build_event_stream, parse_cursor, run_ws_loop};
use crate::state::AppState;
use axum::Json;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use fuxi_core::TaskId;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

/// 解析路径里的 task id——接受裸 UUID 或 `task-<uuid>` Display 形式。
/// 前者给 PWA 短 URL 用；后者兼容 `AgentId::to_string()` 同款契约。
fn parse_task_id(s: &str) -> std::result::Result<TaskId, String> {
    let trimmed = s.strip_prefix("task-").unwrap_or(s);
    Uuid::parse_str(trimmed)
        .map(TaskId::from)
        .map_err(|e| format!("task id 不是合法的 UUID: {s} ({e})"))
}

/// `?from=<cursor>` 公共 query。
#[derive(Debug, Default, Deserialize)]
pub struct StreamQuery {
    /// 回放游标——事件 UUID 或 RFC3339 时间戳。缺省 = live-only。
    pub from: Option<String>,
}

/// `WS /api/conv` —— 玄女对话事件流。
#[tracing::instrument(skip(ws, state))]
pub async fn conv_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Result<Response> {
    let cursor = parse_cursor(q.from.as_deref())?;
    let xuannv = state.fuxi.xuannv_id().await.ok_or_else(|| {
        // 玄女还没起——前端应能区分"暂时无对话流"和"路由错"。返 503 比 400 合适，
        // 但本 crate 错误枚举只有 NOT_FOUND/UNAUTHORIZED/...，先归 NotFound 语义最近。
        Error::NotFound("玄女尚未注入；请先 set_xuannv".into())
    })?;
    info!(?cursor, %xuannv, "ws /api/conv accept");

    let bus = state.fuxi.bus().clone();
    let stream = build_event_stream(&bus, cursor);

    let resp = ws.on_upgrade(move |socket| async move {
        run_ws_loop(socket, stream, move |ev| ev.meta.agent == Some(xuannv)).await;
    });
    Ok(resp)
}

/// `GET /api/conv/messages` —— IM 层聊天记录拉取（Task #17）。
///
/// 与 `WS /api/conv` 的实时事件流互补：WS 推未来发生的，本端点拉**历史**消息。
/// 前端 LoginView 后首屏加载、用户向上滚动加载更早消息都走这里。
#[derive(Debug, Default, Deserialize)]
pub struct MessagesQuery {
    /// 指定 conversation scope。默认 `"xuannv"` 主线；任务子线传 `"task:<task_id>"`。
    #[serde(default)]
    pub conv: Option<String>,
    /// 一页最多多少条；默认 50，硬上限 200（store 内部 clamp）。
    #[serde(default)]
    pub limit: Option<usize>,
    /// 翻历史 cursor —— 上一页 oldest message id。`None` = 最新一页。
    #[serde(default)]
    pub before: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub messages: Vec<Message>,
    pub has_more: bool,
    /// 本页最早消息 id；`None` 表示空页。前端记下当作下次 `before`。
    pub oldest: Option<String>,
}

pub async fn messages(
    State(state): State<AppState>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<MessagesResponse>> {
    let store = state.conv_store.as_ref().ok_or_else(|| {
        Error::Unavailable("conv_store 未注入：daemon 启动期 with_conv_store".into())
    })?;
    let scope = q.conv.as_deref().unwrap_or(SCOPE_XUANNV);
    let conv_id = store.ensure_scope(scope, None).await?;
    let limit = q.limit.unwrap_or(50);
    let (messages, has_more, oldest) = store
        .page_messages(&conv_id, limit, q.before.as_deref())
        .await?;
    Ok(Json(MessagesResponse {
        messages,
        has_more,
        oldest,
    }))
}

/// `WS /api/tasks/{id}/stream` —— 单任务事件流。
#[tracing::instrument(skip(ws, state), fields(task_id = %id))]
pub async fn task_stream_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Result<Response> {
    let task_id = parse_task_id(&id).map_err(|e| {
        warn!(raw = %id, error = %e, "task_id 解析失败");
        Error::BadRequest(e)
    })?;
    let cursor = parse_cursor(q.from.as_deref())?;
    info!(?cursor, "ws /api/tasks/{id}/stream accept");

    let bus = state.fuxi.bus().clone();
    let stream = build_event_stream(&bus, cursor);

    let resp = ws.on_upgrade(move |socket| async move {
        run_ws_loop(socket, stream, move |ev| ev.meta.task == Some(task_id)).await;
    });
    Ok(resp)
}

#[cfg(test)]
mod tests {
    //! 只覆盖 `messages` HTTP handler 的契约——WS handler 走 tests/ws_stream.rs 的
    //! e2e 真 socket 测试。

    use super::*;
    use crate::conv_store::ConvStore;
    use crate::db::init_at;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get as axum_get;
    use chrono::Utc;
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

    async fn build_app() -> (tempfile::TempDir, Router, ConvStore) {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(&dir.path().join("im.db")).await.expect("init");
        let store = ConvStore::new(pool);

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_conv_store(store.clone());
        let app = Router::new()
            .route("/api/conv/messages", axum_get(super::messages))
            .with_state(state);
        (dir, app, store)
    }

    fn req(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn empty_returns_empty_array_no_more() {
        let (_dir, app, _store) = build_app().await;
        let resp = app
            .oneshot(req("/api/conv/messages?conv=xuannv&limit=20"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed["messages"].as_array().unwrap().is_empty());
        assert_eq!(parsed["has_more"], false);
        assert!(parsed["oldest"].is_null());
    }

    #[tokio::test]
    async fn paginates_with_before_cursor() {
        let (_dir, app, store) = build_app().await;
        let conv_id = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let base = Utc::now();
        for i in 0..5 {
            store
                .append_message(
                    &conv_id,
                    "user",
                    None,
                    "text",
                    &serde_json::json!({"text": format!("m-{i}")}),
                    None,
                    None,
                    base + chrono::Duration::milliseconds(i),
                )
                .await
                .unwrap();
        }

        // 拿最新 3 条
        let resp = app
            .clone()
            .oneshot(req("/api/conv/messages?conv=xuannv&limit=3"))
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msgs = parsed["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(parsed["has_more"], true);
        let oldest = parsed["oldest"].as_str().unwrap().to_string();

        // 翻历史
        let url = format!("/api/conv/messages?conv=xuannv&limit=3&before={oldest}");
        let resp = app.oneshot(req(&url)).await.unwrap();
        let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msgs = parsed["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2, "更早只剩 2 条");
        assert_eq!(parsed["has_more"], false);
    }

    #[tokio::test]
    async fn returns_503_when_store_not_injected() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi); // 默认无 conv_store
        let app = Router::new()
            .route("/api/conv/messages", axum_get(super::messages))
            .with_state(state);
        let resp = app.oneshot(req("/api/conv/messages")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
