//! Firehose 客户端 —— TUI 与未来 Rust 消费者共用的订阅入口。
//!
//! 设计取舍：
//! - 两个函数只建立连接、返回解析后的 `Event` 流；断线重连交给调用方的策略层
//!   （现网可能要指数退避 + 带回放 id 的再订阅），P1 先做最简单的一条。
//! - WS 走 `tokio-tungstenite`——text frame 直接 `serde_json::from_str`。
//! - SSE 走 `reqwest` 的字节流——自己按 `\n\n` 切包，比拉个 `eventsource-client`
//!   多写 20 行但零额外依赖。

use crate::error::{Error, Result};
use futures_util::Stream;
use futures_util::stream::StreamExt;
use fuxi_core::Event;
use std::pin::Pin;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};
use url::Url;

/// `Stream<Item = Result<Event>>` 的类型别名——跨模块签名稳定。
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event>> + Send>>;

/// 通过 WebSocket 订阅一条 Hub 事件流。
///
/// 调用方负责：
/// - 在 URL 查询字符串里放 `from_id`/`from_time`（可选）；
/// - 断线后的重连与回放游标携带（P1 不处理）。
#[tracing::instrument(skip_all, fields(url = %url))]
pub async fn connect_ws(url: Url) -> Result<EventStream> {
    info!("ws: 连接");
    let (ws, resp) = connect_async(url.as_str()).await?;
    debug!(status = ?resp.status(), "ws: 握手完成");

    let (_writer, reader) = ws.split();
    let stream = reader.filter_map(|msg| async move {
        match msg {
            Ok(Message::Text(t)) => match serde_json::from_str::<Event>(&t) {
                Ok(ev) => Some(Ok(ev)),
                Err(e) => {
                    warn!(error = %e, "ws: 事件 JSON 解析失败");
                    Some(Err(Error::from(e)))
                }
            },
            Ok(Message::Binary(_)) => {
                // 契约是 text——忽略 binary 防 noise。
                None
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => None,
            Ok(Message::Close(_)) => {
                debug!("ws: server 发送 close");
                None
            }
            Err(e) => Some(Err(Error::from(e))),
        }
    });

    Ok(Box::pin(stream))
}

/// 通过 SSE 订阅一条 Hub 事件流。
#[tracing::instrument(skip_all, fields(url = %url))]
pub async fn connect_sse(url: Url) -> Result<EventStream> {
    info!("sse: 连接");
    let client = reqwest::Client::builder()
        // SSE 长连——不能被全局 timeout 干掉。
        .build()?;
    let resp = client
        .get(url.as_str())
        .header("Accept", "text/event-stream")
        .send()
        .await?
        .error_for_status()?;
    debug!(status = %resp.status(), "sse: 已连上");

    let byte_stream = resp.bytes_stream();
    let event_stream = parse_sse_stream(byte_stream);
    Ok(Box::pin(event_stream))
}

/// 把 `Bytes` 流切成 `Event` 流——按 SSE 规范的 `\n\n` 分隔。
///
/// 为什么不抽到公共 util：P1 SSE 的消费者只有 TUI，先局部内聚；如果再出现第二个
/// 消费者（比如 web dashboard proxy）再提升。
fn parse_sse_stream<S>(byte_stream: S) -> impl Stream<Item = Result<Event>> + Send
where
    S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut buf = String::new();
        tokio::pin!(byte_stream);
        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(b) => b,
                Err(e) => {
                    yield Err(Error::from(e));
                    return;
                }
            };
            // UTF-8 lossy：SSE 规范就是 UTF-8 文本。
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(idx) = find_event_boundary(&buf) {
                // `..idx` 是完整一帧（不含分隔空行），`boundary_len` 是 `\n\n` 或 `\r\n\r\n`。
                let (frame, rest) = buf.split_at(idx);
                let frame = frame.to_string();
                let boundary_len = boundary_len(&buf, idx);
                buf = rest[boundary_len..].to_string();

                if let Some(data) = extract_sse_data(&frame) {
                    match serde_json::from_str::<Event>(&data) {
                        Ok(ev) => yield Ok(ev),
                        Err(e) => {
                            warn!(error = %e, frame = %data, "sse: 事件 JSON 解析失败");
                            yield Err(Error::from(e));
                        }
                    }
                }
                // comment-only / keep-alive 帧（`: ...`）不 yield。
            }
        }
    }
}

fn find_event_boundary(s: &str) -> Option<usize> {
    // 优先认 `\n\n`——axum SSE 用 LF。
    let lf = s.find("\n\n");
    let crlf = s.find("\r\n\r\n");
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn boundary_len(s: &str, idx: usize) -> usize {
    if s[idx..].starts_with("\r\n\r\n") {
        4
    } else {
        2
    }
}

/// 聚合同一帧里的所有 `data:` 行（SSE 允许多行）。
fn extract_sse_data(frame: &str) -> Option<String> {
    let mut out = String::new();
    let mut any = false;
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if any {
                out.push('\n');
            }
            out.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            any = true;
        }
    }
    if any { Some(out) } else { None }
}

// ── async_stream ──
// 为什么直接用 async_stream 的 macro：手写 pin + poll 的 Stream 在 rustc 2024 edition 下
// 会绊到 Send 边界——async-stream crate 已在 workspace 里（fuxi-a2a 用它）。
// 通过 path 引入避免再改 workspace Cargo.toml。
#[allow(dead_code)]
mod async_stream {
    pub use ::async_stream::stream;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_sse_data_single_line() {
        let frame = "data: hello";
        assert_eq!(extract_sse_data(frame).as_deref(), Some("hello"));
    }

    #[test]
    fn extract_sse_data_multi_line() {
        let frame = "data: line1\ndata: line2";
        assert_eq!(extract_sse_data(frame).as_deref(), Some("line1\nline2"));
    }

    #[test]
    fn extract_sse_data_skips_comments() {
        let frame = ": keepalive\ndata: real";
        assert_eq!(extract_sse_data(frame).as_deref(), Some("real"));
    }

    #[test]
    fn extract_sse_data_none_when_only_comment() {
        let frame = ": nothing here";
        assert_eq!(extract_sse_data(frame), None);
    }

    #[test]
    fn find_event_boundary_detects_lf() {
        let s = "data: a\n\ndata: b";
        assert_eq!(find_event_boundary(s), Some(7));
    }

    #[test]
    fn find_event_boundary_detects_crlf() {
        let s = "data: a\r\n\r\ndata: b";
        assert_eq!(find_event_boundary(s), Some(7));
    }
}
