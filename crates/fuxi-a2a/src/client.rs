//! 基于 reqwest 的 A2A 客户端。
//!
//! 之所以不走 hyper raw——A2ARouter 需要和 cc/codex/gemini-cli 这些
//! 部署在用户机器上的外部 agent 互通，reqwest 自带 redirect、proxy、
//! timeout 等生产级行为，没必要自己搓。

use crate::error::{Error, Result};
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse, method};
use crate::sse::{EVENT_ARTIFACT, EVENT_STATUS, ServerSentEventPayload};
use crate::types::{
    AgentCard, SendTaskRequest, SendTaskResponse, Task, TaskArtifactUpdateEvent,
    TaskStatusUpdateEvent,
};
use futures_util::{Stream, StreamExt};
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::pin::Pin;
use uuid::Uuid;

/// A2A 客户端——单连接、线程安全；生产环境可以 clone 到多处。
#[derive(Debug, Clone)]
pub struct A2AClient {
    endpoint: Url,
    http: reqwest::Client,
}

impl A2AClient {
    pub fn new(endpoint: impl AsRef<str>) -> Result<Self> {
        let url = Url::parse(endpoint.as_ref())
            .map_err(|e| Error::InvalidUrl(format!("{}: {e}", endpoint.as_ref())))?;
        Ok(Self {
            endpoint: url,
            http: reqwest::Client::new(),
        })
    }

    pub fn with_http(endpoint: impl AsRef<str>, http: reqwest::Client) -> Result<Self> {
        let url = Url::parse(endpoint.as_ref())
            .map_err(|e| Error::InvalidUrl(format!("{}: {e}", endpoint.as_ref())))?;
        Ok(Self {
            endpoint: url,
            http,
        })
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_agent_card(&self) -> Result<AgentCard> {
        self.call(method::AGENT_GET_CARD, json!({})).await
    }

    #[tracing::instrument(skip(self, req))]
    pub async fn send_task(&self, req: SendTaskRequest) -> Result<Task> {
        let resp: SendTaskResponse = self.call(method::TASKS_SEND, json!(req)).await?;
        Ok(resp.task)
    }

    #[tracing::instrument(skip(self))]
    pub async fn get_task(&self, id: &str) -> Result<Task> {
        let resp: SendTaskResponse = self.call(method::TASKS_GET, json!({ "id": id })).await?;
        Ok(resp.task)
    }

    #[tracing::instrument(skip(self))]
    pub async fn cancel_task(&self, id: &str) -> Result<Task> {
        let resp: SendTaskResponse = self.call(method::TASKS_CANCEL, json!({ "id": id })).await?;
        Ok(resp.task)
    }

    /// SSE 订阅：返回一个 `Stream<Item = Result<ServerSentEventPayload>>`。
    /// 流在收到 `status.final == true` 或 HTTP 连接关闭时结束。
    #[tracing::instrument(skip(self, req))]
    pub async fn send_task_subscribe(
        &self,
        req: SendTaskRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ServerSentEventPayload>> + Send>>> {
        let envelope = JsonRpcRequest::new(
            Uuid::new_v4().to_string(),
            method::TASKS_SEND_SUBSCRIBE,
            json!(req),
        );
        let resp = self
            .http
            .post(self.endpoint.clone())
            .header("accept", "text/event-stream")
            .json(&envelope)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::A2A(format!("sendSubscribe http {status}: {body}")));
        }
        // reqwest 的 `bytes_stream` 已经是按 TCP chunk 切分——我们在上面套一层
        // SSE frame parser，按 `\n\n` 分割并抽取 `event:` / `data:` 字段。
        let byte_stream = resp.bytes_stream();
        let parsed = parse_sse_stream(byte_stream);
        Ok(Box::pin(parsed))
    }

    /// 内部：通用 JSON-RPC 调用。
    async fn call<R: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<R> {
        let envelope = JsonRpcRequest::new(Uuid::new_v4().to_string(), method, params);
        let resp = self
            .http
            .post(self.endpoint.clone())
            .json(&envelope)
            .send()
            .await?;
        let body: JsonRpcResponse = resp.json().await?;
        if let Some(err) = body.error {
            return Err(Error::JsonRpc {
                code: err.code,
                message: err.message,
            });
        }
        let result = body
            .result
            .ok_or_else(|| Error::A2A("response missing both result and error".into()))?;
        Ok(serde_json::from_value(result)?)
    }
}

/// 把 reqwest 的 bytes stream 解析成 A2A SSE payload 流。
///
/// SSE 帧解析规则：
/// - 以 `\n\n` 为帧分隔符；
/// - 每帧内部按行拆分，`event: X` 行指定事件名（默认 `message`）；
/// - 多个 `data:` 行拼接（用 `\n`），最终作为 payload JSON。
fn parse_sse_stream<S>(byte_stream: S) -> impl Stream<Item = Result<ServerSentEventPayload>> + Send
where
    S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut buf = String::new();
        let mut bs = Box::pin(byte_stream);
        while let Some(chunk) = bs.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    yield Err(Error::from(e));
                    break;
                }
            };
            match std::str::from_utf8(&chunk) {
                Ok(s) => buf.push_str(s),
                Err(e) => {
                    yield Err(Error::A2A(format!("non-utf8 sse chunk: {e}")));
                    break;
                }
            }
            // 处理所有已完整到达的 `\n\n` 帧。
            while let Some(idx) = buf.find("\n\n") {
                let frame = buf[..idx].to_string();
                buf.drain(..idx + 2);
                match decode_sse_frame(&frame) {
                    Ok(Some(payload)) => yield Ok(payload),
                    Ok(None) => continue, // 空帧或注释（冒号开头），跳过
                    Err(e) => yield Err(e),
                }
            }
        }
    }
}

fn decode_sse_frame(frame: &str) -> Result<Option<ServerSentEventPayload>> {
    let mut event_name: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in frame.split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start_matches(' '));
        }
    }
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let name = event_name.unwrap_or_else(|| "message".into());
    match name.as_str() {
        EVENT_STATUS => {
            let v: TaskStatusUpdateEvent = serde_json::from_str(&data)?;
            Ok(Some(ServerSentEventPayload::Status(v)))
        }
        EVENT_ARTIFACT => {
            let v: TaskArtifactUpdateEvent = serde_json::from_str(&data)?;
            Ok(Some(ServerSentEventPayload::Artifact(v)))
        }
        other => Err(Error::A2A(format!("unknown sse event type: {other}"))),
    }
}
