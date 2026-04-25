//! `/api/conv` + `/api/tasks/:id/stream` —— WebSocket 事件流 stub。
//!
//! γ 实装时把 WS upgrade + EventBus 订阅塞进来，参考 `fuxi-firehose/src/hub.rs::ws_loop`。
//! 骨架阶段：返 501 让前端能识别"端点存在但未实装"，区分于 404。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::extract::{Path, State};

pub async fn conv_ws(State(_state): State<AppState>) -> Result<&'static str> {
    Err(Error::NotImplemented("WS /api/conv"))
}

pub async fn task_stream_ws(
    State(_state): State<AppState>,
    Path(_id): Path<String>,
) -> Result<&'static str> {
    Err(Error::NotImplemented("WS /api/tasks/:id/stream"))
}
