//! A2A `sendSubscribe` 走 SSE——本模块把 "事件类型 + JSON payload" 抽象成
//! 统一的 `ServerSentEvent`，让 server/client 两端都只跟这个类型打交道。
//!
//! 为什么不直接把 axum 的 SSE event 暴露到 trait：`A2AService` 要做到
//! 跟 HTTP 框架解耦（单元测试、将来换 hyper-based 实现），所以 trait
//! 只产出我们自己的 `ServerSentEvent`，由 server 模块负责翻译到 axum。

use serde::{Deserialize, Serialize};

/// Server 向 client 发出的一条 SSE 事件。`event` 字段对应 SSE 协议的
/// `event:` 行，`data` 是 JSON 编码的 payload。
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSentEvent {
    pub event: String,
    pub data: serde_json::Value,
}

/// client 从 SSE 流解码出的事件 payload。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerSentEventPayload {
    /// `tasks/sendSubscribe` 的状态变化事件（`event: status`）。
    Status(crate::types::TaskStatusUpdateEvent),
    /// artifact 增量（`event: artifact`）。
    Artifact(crate::types::TaskArtifactUpdateEvent),
}

pub const EVENT_STATUS: &str = "status";
pub const EVENT_ARTIFACT: &str = "artifact";

impl ServerSentEvent {
    pub fn status(update: &crate::types::TaskStatusUpdateEvent) -> crate::Result<Self> {
        Ok(Self {
            event: EVENT_STATUS.into(),
            data: serde_json::to_value(update)?,
        })
    }

    pub fn artifact(update: &crate::types::TaskArtifactUpdateEvent) -> crate::Result<Self> {
        Ok(Self {
            event: EVENT_ARTIFACT.into(),
            data: serde_json::to_value(update)?,
        })
    }
}
