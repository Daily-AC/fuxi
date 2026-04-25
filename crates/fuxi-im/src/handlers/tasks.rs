//! `/api/tasks*`——任务卡片列表 + 单任务事件历史。
//!
//! 骨架阶段：`list_tasks` 返空 JSON 数组（PWA 起来时主屏先有可渲染的 200 响应，
//! 而不是 501，避免误以为整条链路坏掉）。真实数据由后续 task tree 集成 owner
//! 接入——先把契约定下来：**永远是数组，永远 200**。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
pub struct EventsQuery {
    /// 回放游标（事件 UUID）。
    pub from: Option<String>,
}

/// `GET /api/tasks/:id/events` —— 单任务事件历史 stub（γ 接管）。
pub async fn task_events(
    State(_state): State<AppState>,
    Path(_id): Path<String>,
    Query(_q): Query<EventsQuery>,
) -> Result<Json<Vec<serde_json::Value>>> {
    Err(Error::NotImplemented("GET /api/tasks/:id/events"))
}
