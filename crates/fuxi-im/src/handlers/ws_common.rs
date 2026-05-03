//! WebSocket 通用循环 —— 玄女对话流 / 单任务流共用。
//!
//! 设计取舍：
//! - 一份事件流构造器（`build_event_stream`）—— `?from=<cursor>` 走 `bus.replay` 拼
//!   live tail；缺省走纯 `bus.subscribe`。和 `fuxi-firehose::hub::build_event_stream`
//!   同款思路（公理 #3：真实时不轮询）。
//! - WS 主循环 `run_ws_loop`：tokio::select! 多路——事件流 / 心跳 ping / 入站 close。
//!   broadcast `Lagged` 由 `bus.subscribe()` 内部 filter 掉（CLAUDE.md 强调不要终止
//!   订阅链）；这里再用 `Filter` 闭包给 caller 做语义过滤（按玄女 / 按 task）。
//! - 客户端断线重连协议跟 firehose 一致——客户端自带 `?from=<last_event_id>` 重连，
//!   服务端不主动续接。
//!
//! 为什么不直接复用 firehose `Hub`：fuxi-im 走 `AppState { fuxi: Arc<Fuxi> }` 注入，
//! 而 firehose `Hub` 内部持的是 `EventBus` + `EventStore`——通过 `state.fuxi.bus()`
//! 拿一份引用就够了，没必要再绕一层 Arc<Hub>。

use crate::error::{Error, Result};
use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use futures_util::stream::{Stream, StreamExt};
use fuxi_core::Event;
use fuxi_events::{EventBus, ReplayCursor};
use serde::Serialize;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

/// 历史事件 HTTP 响应包装——`task_events` / `worker_events` 共用。
///
/// **wire 形契约对齐**（P0.D #45 修）：前端 `EventHistoryResponse`（`web/src/types/events.ts:87`）
/// 期望 `{events, next_cursor}`，e2e/mock fixture 也按此 shape 写。
/// 早期 backend 直接返 `Vec<Event>` 裸数组 → 前端 `r.events` undefined → reduce
/// throw → catch 静默 → 历史回放永远空。task done 后 WS 也不再来事件，
/// 用户感知"门客原本输出全没了"。
///
/// `next_cursor` 当前永远 `None`（无服务端分页），保留字段为后续 limit/cursor 翻页留口。
#[derive(Debug, Serialize)]
pub(crate) struct EventHistoryResponse {
    pub events: Vec<Event>,
    pub next_cursor: Option<String>,
}

/// WebSocket 心跳间隔。和 firehose 保持一致，nginx idle timeout 留余量。
pub(crate) const WS_PING_INTERVAL: Duration = Duration::from_secs(15);

/// 事件流类型——`build_event_stream` 的返回类型别名。
pub(crate) type EventFlow = Pin<Box<dyn Stream<Item = Result<Event>> + Send>>;

/// `?from=<value>` cursor 解析。
///
/// 接受两种形式：UUID（按 event id）或 RFC3339 时间戳。和 firehose `from_id` /
/// `from_time` 拆两个 query key 不同——IM 决策 14 C 节路由表里 query 写的就是单
/// `from`，前端只需带最后一个事件的 id 即可重连，体验更简单。
pub(crate) fn parse_cursor(from: Option<&str>) -> Result<Option<ReplayCursor>> {
    let Some(s) = from else { return Ok(None) };
    if s.is_empty() {
        return Ok(None);
    }
    if let Ok(u) = Uuid::parse_str(s) {
        return Ok(Some(ReplayCursor::FromId(u)));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(Some(ReplayCursor::FromTime(dt.with_timezone(&Utc))));
    }
    Err(Error::BadRequest(format!(
        "cursor `from={s}` 应为 UUID 或 RFC3339 时间戳"
    )))
}

/// 构造统一的"history + live tail"事件流。
///
/// 行为和 firehose 一致：cursor 缺省 → 纯 live；提供 → 历史 + tail。
pub(crate) fn build_event_stream(bus: &EventBus, cursor: Option<ReplayCursor>) -> EventFlow {
    match cursor {
        Some(c) => Box::pin(bus.replay(c, true).map(|r| r.map_err(Error::from))),
        None => Box::pin(bus.subscribe().map(|r| r.map_err(Error::from))),
    }
}

/// WS 主循环——按 `filter` 决定哪些 `Event` 推给客户端。
///
/// `filter` 返回 true 才下发；否则跳过——典型场景：
///   - `/api/conv`：只下发 `meta.agent == xuannv_id`
///   - `/api/tasks/{id}/stream`：只下发 `meta.task == Some(id)`
///
/// 心跳 + 入站 close 处理同 firehose `ws_loop`——`Lagged` 在 bus 层 filter 已忽略，
/// 这里再次保险：事件流抛错只 `warn!` 不 break，下一次 tick 继续。
pub(crate) async fn run_ws_loop<F>(mut socket: WebSocket, mut events: EventFlow, filter: F)
where
    F: Fn(&Event) -> bool + Send + 'static,
{
    let mut ping = tokio::time::interval(WS_PING_INTERVAL);
    // 跳过首次立即 tick——客户端刚连上来不需要立刻 ping。
    ping.tick().await;

    loop {
        tokio::select! {
            maybe_ev = events.next() => {
                match maybe_ev {
                    Some(Ok(ev)) => {
                        if !filter(&ev) {
                            continue;
                        }
                        let json = match serde_json::to_string(&ev) {
                            Ok(s) => s,
                            Err(e) => {
                                // 极少发生——payload 已经从 SQLite/broadcast 拿出来，
                                // 这里失败说明 EventKind serde 自身有 bug。
                                warn!(error = %e, "ws: 事件序列化失败，跳过");
                                continue;
                            }
                        };
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            debug!("ws: peer 已断开");
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        // bus 层错误——记一下继续。subscribe stream 自身不会因
                        // Lagged 终止（已在 bus.rs::subscribe filter 掉），其他错误
                        // 也不应让整个 ws 退出（比如 store backend 的瞬时 BUSY）。
                        warn!(error = %e, "ws: 事件流错误，继续");
                    }
                    None => {
                        debug!("ws: 事件流结束");
                        break;
                    }
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    debug!("ws: ping 失败，peer 已断开");
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("ws: 收到 close 或流关闭");
                        break;
                    }
                    Some(Ok(_)) => {
                        // text/binary/pong 一律忽略——本接口只推不收。
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "ws: 入站流错误，关闭连接");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cursor_none_returns_none() {
        assert!(parse_cursor(None).expect("ok").is_none());
        assert!(parse_cursor(Some("")).expect("ok").is_none());
    }

    #[test]
    fn parse_cursor_uuid() {
        let u = Uuid::new_v4();
        let c = parse_cursor(Some(&u.to_string())).expect("ok");
        assert!(matches!(c, Some(ReplayCursor::FromId(_))));
    }

    #[test]
    fn parse_cursor_rfc3339() {
        let c = parse_cursor(Some("2026-04-25T00:00:00Z")).expect("ok");
        assert!(matches!(c, Some(ReplayCursor::FromTime(_))));
    }

    #[test]
    fn parse_cursor_garbage_is_bad_request() {
        let err = parse_cursor(Some("not-a-uuid-or-time")).expect_err("err");
        assert!(matches!(err, Error::BadRequest(_)));
    }
}
