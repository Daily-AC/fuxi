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

use crate::error::{Error, Result};
use crate::handlers::ws_common::{build_event_stream, parse_cursor, run_ws_loop};
use crate::state::AppState;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use fuxi_core::TaskId;
use serde::Deserialize;
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
