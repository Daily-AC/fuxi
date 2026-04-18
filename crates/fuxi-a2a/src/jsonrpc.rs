//! A2A 采用 JSON-RPC 2.0 信封——本模块只关心 envelope，不触碰 method payload。
//!
//! 设计取舍：A2A 规范 v1.0 将 streaming 方法 `tasks/sendSubscribe` 的结果
//! 通过 SSE 推送，因此 envelope 只覆盖 request 和非流式 response；流式
//! 事件走 `sse` 模块另行处理。

use serde::{Deserialize, Serialize};

pub const JSONRPC_VERSION: &str = "2.0";

/// A2A 方法名常量——避免散落在各处写字符串字面量带来的拼写错。
pub mod method {
    pub const AGENT_GET_CARD: &str = "agent/getCard";
    pub const TASKS_SEND: &str = "tasks/send";
    pub const TASKS_SEND_SUBSCRIBE: &str = "tasks/sendSubscribe";
    pub const TASKS_GET: &str = "tasks/get";
    pub const TASKS_CANCEL: &str = "tasks/cancel";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    /// JSON-RPC 允许 id 为字符串/数字/null——统一用 Value 吃掉多态。
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: impl Into<serde_json::Value>, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id: id.into(),
            method: method.into(),
            params: Some(params),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: serde_json::Value, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}
