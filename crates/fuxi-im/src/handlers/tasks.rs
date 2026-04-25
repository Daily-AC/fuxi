//! `/api/tasks*`——任务卡片列表 + 单任务事件历史。
//!
//! `list_tasks`（α 留的 stub）：暂返空 JSON 数组，**契约：永远是数组、永远 200**。
//! 真实数据等 task tree owner 接入；γ 不动。
//!
//! `task_events`（γ 实装）：单任务历史事件回放——HTTP 同步端点，**不 tail**。
//! 实时订阅请走 `WS /api/tasks/{id}/stream`（公理 #3：真实时不轮询）。
//! cursor 缺省 → 该 task 全量历史；带 `?from=<event_id|rfc3339>` → 严格之后。
//! 默认 limit=100，硬上限 1000，防止前端误打分页接口当 dump 工具。

use crate::error::{Error, Result};
use crate::handlers::ws_common::parse_cursor;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use futures_util::StreamExt;
use fuxi_core::{Event, TaskId};
use fuxi_events::ReplayCursor;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 同 `handlers/conv.rs::parse_task_id`：URL path 段允许裸 UUID 或 `task-<uuid>`。
fn parse_task_id(s: &str) -> std::result::Result<TaskId, String> {
    let trimmed = s.strip_prefix("task-").unwrap_or(s);
    Uuid::parse_str(trimmed)
        .map(TaskId::from)
        .map_err(|e| format!("task id 不是合法的 UUID: {s} ({e})"))
}

// `#[allow(dead_code)]`：字段在骨架阶段没人读，等 owner 接入 task tree 时才会用。
// 留 `#[derive(Deserialize)]` 让 axum 路由把 query 解出来，后续直接用。
#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
pub struct ListTasksQuery {
    /// `root=1` → 只返 root 任务（主屏卡片）；缺省视为同 1。
    pub root: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct TaskCard {
    pub id: String,
    pub title: String,
    pub state: String,
}

/// `GET /api/tasks?root=1` —— 主屏卡片列表 stub。
pub async fn list_tasks(
    State(_state): State<AppState>,
    Query(_q): Query<ListTasksQuery>,
) -> Result<Json<Vec<TaskCard>>> {
    Ok(Json(Vec::new()))
}

/// `?from=<cursor>&limit=N` 历史回放查询。
#[derive(Debug, Default, Deserialize)]
pub struct EventsQuery {
    /// 回放起点：事件 UUID 或 RFC3339 时间戳。缺省 = 该 task 历史从头。
    pub from: Option<String>,
    /// 最多返回条数；默认 100，最大 1000。
    pub limit: Option<usize>,
}

/// `GET /api/tasks/:id/events?from=<cursor>&limit=N` —— 单任务事件历史。
///
/// 使用 `EventStore::replay` 拉全表流然后按 `meta.task == :id` 过滤+分页——
/// 不直接走 `history_for_task` 是因为后者无 cursor 语义。`replay(FromId)` 走
/// rowid 锚点，跨任务统一时间序保持单调。
#[tracing::instrument(skip(state), fields(task_id = %id))]
pub async fn task_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<Event>>> {
    let task_id = parse_task_id(&id).map_err(Error::BadRequest)?;
    let cursor = parse_cursor(q.from.as_deref())?.unwrap_or(ReplayCursor::Beginning);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);

    let store = state.fuxi.bus().store().clone();
    let mut stream = store.replay(cursor);
    let mut out: Vec<Event> = Vec::with_capacity(limit.min(256));

    while let Some(item) = stream.next().await {
        let ev = item?;
        if ev.meta.task != Some(task_id) {
            continue;
        }
        out.push(ev);
        if out.len() >= limit {
            break;
        }
    }
    Ok(Json(out))
}
