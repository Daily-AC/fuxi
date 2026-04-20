//! axum-based A2A server——所有方法复用同一个 `/a2a` POST 端点，
//! 按 JSON-RPC `method` 字段路由；流式方法额外接受 SSE `GET /a2a/stream`
//! 以及 POST `/a2a` 内 `tasks/sendSubscribe` 的结果。
//!
//! 设计取舍：A2A 规范允许 server 选择 "同一端点复用" 或 "多端点" 两种形态；
//! 本实现遵循 "单一 /a2a POST 分发" 的简洁路线，同时为 SSE 流 **保留同一
//! 个端点**——当 method 是 sendSubscribe 时直接升级为 `text/event-stream`
//! 响应，免去了客户端再打开一条连接的复杂度。

use crate::error::{Error, Result};
use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, method};
use crate::sse::ServerSentEvent;
use crate::wire::{AgentCard, SendTaskRequest, Task};
use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event as AxumSse, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::stream::BoxStream;
use futures_util::{StreamExt, stream};
use std::convert::Infallible;
use std::sync::Arc;

/// 具体 agent 的业务契约——每个 `cc/codex/gemini-cli` 适配器实现一份，
/// A2A server 只负责协议层。
#[async_trait]
pub trait A2AService: Send + Sync + 'static {
    async fn agent_card(&self) -> Result<AgentCard>;
    async fn send_task(&self, req: SendTaskRequest) -> Result<Task>;
    /// 流式响应：由实现方决定往流里推多少 status/artifact 更新；
    /// 规范要求最后一条必然是 `final: true` 的 status 事件。
    async fn send_task_subscribe(
        &self,
        req: SendTaskRequest,
    ) -> Result<BoxStream<'static, ServerSentEvent>>;
    async fn get_task(&self, id: &str) -> Result<Task>;
    async fn cancel_task(&self, id: &str) -> Result<Task>;
}

/// 构造 axum Router。`service` 以 `Arc<S>` 形式共享到每个 handler。
pub fn router<S: A2AService>(service: Arc<S>) -> Router {
    Router::new()
        .route("/a2a", post(dispatch::<S>))
        .with_state(service)
}

/// 单入口 POST handler：先解析 envelope，再按 method 分发。
///
/// 返回类型是 `axum::response::Response`——非流式走 JSON，流式走 SSE，
/// 两种形态通过 `IntoResponse` 统一到同一个 handler 签名。
#[tracing::instrument(skip_all, fields(method))]
async fn dispatch<S: A2AService>(
    State(service): State<Arc<S>>,
    body: axum::body::Bytes,
) -> Response {
    let req: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return json_err(
                serde_json::Value::Null,
                Error::CODE_PARSE,
                format!("parse: {e}"),
                StatusCode::BAD_REQUEST,
            );
        }
    };
    tracing::Span::current().record("method", req.method.as_str());

    match req.method.as_str() {
        method::AGENT_GET_CARD => handle_simple(req.id, service.agent_card().await).await,
        method::TASKS_SEND => {
            let params: SendTaskRequest = match parse_params(req.params.clone()) {
                Ok(v) => v,
                Err(e) => return jsonrpc_error(req.id, e),
            };
            handle_send(req.id, service.send_task(params).await).await
        }
        method::TASKS_GET => {
            let id = match parse_id_param(req.params.clone()) {
                Ok(v) => v,
                Err(e) => return jsonrpc_error(req.id, e),
            };
            handle_send(req.id, service.get_task(&id).await).await
        }
        method::TASKS_CANCEL => {
            let id = match parse_id_param(req.params.clone()) {
                Ok(v) => v,
                Err(e) => return jsonrpc_error(req.id, e),
            };
            handle_send(req.id, service.cancel_task(&id).await).await
        }
        method::TASKS_SEND_SUBSCRIBE => {
            let params: SendTaskRequest = match parse_params(req.params.clone()) {
                Ok(v) => v,
                Err(e) => return jsonrpc_error(req.id, e),
            };
            match service.send_task_subscribe(params).await {
                Ok(s) => into_sse(s).into_response(),
                Err(e) => jsonrpc_error(req.id, e),
            }
        }
        other => json_err(
            req.id,
            Error::CODE_METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
            StatusCode::OK,
        ),
    }
}

fn parse_params<T: for<'de> serde::Deserialize<'de>>(
    params: Option<serde_json::Value>,
) -> Result<T> {
    let v = params.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(v).map_err(Error::from)
}

/// `tasks/get` 和 `tasks/cancel` 只带一个 `{ "id": "..." }` 参数——
/// 规范允许直接传 `"t-1"` 字符串或 `{"id":"t-1"}` 对象，这里两种都接受。
fn parse_id_param(params: Option<serde_json::Value>) -> Result<String> {
    let v = params.unwrap_or(serde_json::Value::Null);
    match v {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Object(mut m) => match m.remove("id") {
            Some(serde_json::Value::String(s)) => Ok(s),
            _ => Err(Error::A2A("missing string `id` in params".into())),
        },
        _ => Err(Error::A2A("expected id string or object".into())),
    }
}

/// 针对返回 `AgentCard` 这种无需二次 serialize 的分支。
async fn handle_simple<T: serde::Serialize>(id: serde_json::Value, r: Result<T>) -> Response {
    match r {
        Ok(v) => {
            let val = match serde_json::to_value(&v) {
                Ok(x) => x,
                Err(e) => return jsonrpc_error(id, Error::from(e)),
            };
            axum::Json(JsonRpcResponse::success(id, val)).into_response()
        }
        Err(e) => jsonrpc_error(id, e),
    }
}

/// 对 `Task` 结果封装进 `SendTaskResponse { task }`，与 A2A 规范对齐。
async fn handle_send(id: serde_json::Value, r: Result<Task>) -> Response {
    match r {
        Ok(task) => {
            let val = match serde_json::to_value(serde_json::json!({ "task": task })) {
                Ok(x) => x,
                Err(e) => return jsonrpc_error(id, Error::from(e)),
            };
            axum::Json(JsonRpcResponse::success(id, val)).into_response()
        }
        Err(e) => jsonrpc_error(id, e),
    }
}

fn jsonrpc_error(id: serde_json::Value, err: Error) -> Response {
    json_err(id, err.jsonrpc_code(), err.to_string(), StatusCode::OK)
}

/// JSON-RPC 语义下服务端通常仍然回 200——错误在 body 的 `error` 字段；
/// 但解析失败阶段（id 都还拿不到）我们给 400，让传输层也能感知。
fn json_err(id: serde_json::Value, code: i32, message: String, http: StatusCode) -> Response {
    let body = JsonRpcResponse::failure(
        id,
        JsonRpcError {
            code,
            message,
            data: None,
        },
    );
    (http, axum::Json(body)).into_response()
}

/// 把业务侧的 `BoxStream<ServerSentEvent>` 翻译成 axum SSE。
/// 保留 keep-alive 防止代理中断。
fn into_sse(
    s: BoxStream<'static, ServerSentEvent>,
) -> Sse<impl futures_util::Stream<Item = std::result::Result<AxumSse, Infallible>>> {
    let mapped = s.map(|ev| {
        let data = ev.data.to_string();
        let axum_ev = AxumSse::default().event(ev.event).data(data);
        Ok::<_, Infallible>(axum_ev)
    });
    Sse::new(mapped).keep_alive(KeepAlive::default())
}

/// helper：业务实现可以直接从一个 `Vec<ServerSentEvent>` 造一条 stream。
pub fn stream_from_events(events: Vec<ServerSentEvent>) -> BoxStream<'static, ServerSentEvent> {
    stream::iter(events).boxed()
}
