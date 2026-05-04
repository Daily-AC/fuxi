//! Firehose Hub —— 通过 WebSocket + SSE 把 `EventBus` 暴露给外部订阅者。
//!
//! 公理 #3 “真实时，不轮询”——外部客户端拿到的一律是推送流，不是轮询端点。
//! 为什么同时支持 WS 和 SSE：TUI/脚手架走 WS（双向心跳更可靠）、浏览器/curl 走
//! SSE（无需升级）——两条路共用同一个 `Event` 序列化契约，互换零成本。
//!
//! REST `/events` 单独存在只是提供历史快照/分页检索，**不会 tail live**——
//! 实时订阅必须走 WS/SSE，严守公理。

use crate::error::{Error, Result};
use axum::Router;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Utc};
use futures_util::stream::{Stream, StreamExt};
use fuxi_core::Event;
use fuxi_events::{EventBus, EventStore, ReplayCursor};
use serde::Deserialize;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Firehose Hub——挂在 axum Router 上的有状态组件。
///
/// clone/共享通过 `Arc<Hub>`；`EventBus` 与 `EventStore` 内部都是 `Arc`，
/// 多 handler 共享一份零成本。
pub struct Hub {
    bus: EventBus,
    store: EventStore,
}

impl Hub {
    /// 用已有的 `EventBus` 构造——`EventStore` 从 bus 取。
    pub fn new(bus: EventBus) -> Self {
        let store = bus.store().clone();
        Self { bus, store }
    }

    /// 暴露给上层的访问器。
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// 暴露给上层的访问器。
    pub fn store(&self) -> &EventStore {
        &self.store
    }
}

/// 订阅/检索的共用查询参数。
///
/// `from_id` / `from_time` 互斥——同时给时以 `from_id` 为准（更精确）。
#[derive(Debug, Default, Deserialize)]
pub struct SubscribeQuery {
    /// 回放起点的事件 id（UUID）。提供时：先拉历史（严格之后该条），再接 live。
    pub from_id: Option<String>,
    /// 回放起点的 RFC3339 时间戳。提供时：先拉该时刻起的历史，再接 live。
    pub from_time: Option<String>,
}

/// REST `/events` 查询参数。
#[derive(Debug, Default, Deserialize)]
pub struct HistoryQuery {
    /// 最多返回条数，默认 100，最大 10_000。
    pub limit: Option<usize>,
    /// 回放起点 id。
    pub from_id: Option<String>,
    /// 回放起点时间。
    pub from_time: Option<String>,
    /// 限定某个 agent——按 `AgentId::to_string()`（`agent-<uuid>`）匹配。
    pub agent: Option<String>,
    /// 限定某种事件类型（`kind_tag` 精确匹配）。
    pub kind: Option<String>,
}

/// 默认历史页大小。
pub const DEFAULT_HISTORY_LIMIT: usize = 100;
/// 硬上限，防止查库被滥用。
pub const MAX_HISTORY_LIMIT: usize = 10_000;
/// WebSocket 心跳间隔；与规格要求保持一致。
pub const WS_PING_INTERVAL: Duration = Duration::from_secs(15);

/// 构造 Hub 的 axum Router。调用方自行 `axum::serve(listener, router)`.
pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/sse", get(sse_handler))
        .route("/events", get(events_handler))
        .with_state(hub)
}

/// 把查询参数转成 `ReplayCursor`；两者都缺则返回 None（live-only）。
fn cursor_from_params(
    from_id: Option<&str>,
    from_time: Option<&str>,
) -> Result<Option<ReplayCursor>> {
    if let Some(id) = from_id {
        let uuid = Uuid::parse_str(id)?;
        return Ok(Some(ReplayCursor::FromId(uuid)));
    }
    if let Some(ts) = from_time {
        let parsed: DateTime<Utc> = DateTime::parse_from_rfc3339(ts)?.with_timezone(&Utc);
        return Ok(Some(ReplayCursor::FromTime(parsed)));
    }
    Ok(None)
}

/// WebSocket upgrade handler。
#[tracing::instrument(skip(ws, hub))]
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(hub): State<Arc<Hub>>,
    Query(q): Query<SubscribeQuery>,
) -> Response {
    let cursor = match cursor_from_params(q.from_id.as_deref(), q.from_time.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "ws: 非法查询参数");
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    info!(?cursor, "ws accept");
    ws.on_upgrade(move |socket| ws_loop(socket, hub, cursor))
}

async fn ws_loop(mut socket: WebSocket, hub: Arc<Hub>, cursor: Option<ReplayCursor>) {
    let mut stream = build_event_stream(&hub.bus, cursor);
    let mut ping = tokio::time::interval(WS_PING_INTERVAL);
    // 第一次 tick 立刻就绪——跳过以避免客户端连上来就被 ping 洗一遍。
    ping.tick().await;

    loop {
        tokio::select! {
            maybe_ev = stream.next() => {
                match maybe_ev {
                    Some(Ok(ev)) => {
                        let json = match serde_json::to_string(&ev) {
                            Ok(s) => s,
                            Err(e) => {
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
                        warn!(error = %e, "ws: 事件流错误");
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
                        // 忽略其他入站消息（Pong / Text / Binary）——本接口只推不收。
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "ws: 入站流错误");
                        break;
                    }
                }
            }
        }
    }
}

/// SSE handler——契约与 WS 一致：每条事件就是一条 `data: <json>` 帧。
#[tracing::instrument(skip(hub))]
async fn sse_handler(State(hub): State<Arc<Hub>>, Query(q): Query<SubscribeQuery>) -> Response {
    let cursor = match cursor_from_params(q.from_id.as_deref(), q.from_time.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "sse: 非法查询参数");
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    info!(?cursor, "sse accept");

    let stream = build_event_stream(&hub.bus, cursor).filter_map(|r| async move {
        match r {
            Ok(ev) => match serde_json::to_string(&ev) {
                Ok(json) => Some(Ok::<_, Infallible>(SseEvent::default().data(json))),
                Err(e) => {
                    warn!(error = %e, "sse: 事件序列化失败，跳过");
                    None
                }
            },
            Err(e) => {
                warn!(error = %e, "sse: 事件流错误");
                None
            }
        }
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// REST 历史端点——仅返回一次性快照，**不 tail**。
#[tracing::instrument(skip(hub))]
async fn events_handler(State(hub): State<Arc<Hub>>, Query(q): Query<HistoryQuery>) -> Response {
    let cursor = match cursor_from_params(q.from_id.as_deref(), q.from_time.as_deref()) {
        Ok(c) => c.unwrap_or(ReplayCursor::Beginning),
        Err(e) => {
            warn!(error = %e, "events: 非法查询参数");
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    let limit = q
        .limit
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
        .clamp(1, MAX_HISTORY_LIMIT);

    let mut stream = hub.store.replay(cursor);
    let mut out: Vec<Event> = Vec::with_capacity(limit.min(1024));

    while let Some(item) = stream.next().await {
        match item {
            Ok(ev) => {
                if let Some(agent_filter) = &q.agent {
                    match &ev.meta.agent {
                        Some(a) if a.to_string() == *agent_filter => {}
                        _ => continue,
                    }
                }
                if let Some(kind_filter) = &q.kind
                    && kind_tag(&ev.kind) != kind_filter.as_str()
                {
                    continue;
                }
                out.push(ev);
                if out.len() >= limit {
                    break;
                }
            }
            Err(e) => {
                warn!(error = %e, "events: 回放流错误");
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
    }

    axum::Json(out).into_response()
}

/// 统一的“history + live tail”流构造器。
///
/// 为什么专门抽出来：WS 和 SSE 都用，行为要完全一致。
fn build_event_stream(
    bus: &EventBus,
    cursor: Option<ReplayCursor>,
) -> Pin<Box<dyn Stream<Item = std::result::Result<Event, Error>> + Send>> {
    match cursor {
        Some(c) => Box::pin(bus.replay(c, true).map(|r| r.map_err(Error::from))),
        None => Box::pin(bus.subscribe().map(|r| r.map_err(Error::from))),
    }
}

/// 把 `EventKind` 的 serde tag 映射成字符串；与 events 层的实现保持一致。
pub(crate) fn kind_tag(kind: &fuxi_core::EventKind) -> &'static str {
    use fuxi_core::EventKind::*;
    match kind {
        AgentSpawning { .. } => "agent_spawning",
        AgentReady { .. } => "agent_ready",
        AgentShuttingDown { .. } => "agent_shutting_down",
        AgentDead { .. } => "agent_dead",
        TaskCreated { .. } => "task_created",
        TaskDispatched { .. } => "task_dispatched",
        TaskStateChanged { .. } => "task_state_changed",
        TaskBlocked { .. } => "task_blocked",
        TaskResumed { .. } => "task_resumed",
        UserPrompted { .. } => "user_prompted",
        AgentResponded { .. } => "agent_responded",
        ThinkingStarted => "thinking_started",
        ThinkingFinished => "thinking_finished",
        ToolCallStarted { .. } => "tool_call_started",
        ToolCallFinished { .. } => "tool_call_finished",
        UserInterventionSent { .. } => "user_intervention_sent",
        AgentInterrupted { .. } => "agent_interrupted",
        TaskInterventionApplied { .. } => "task_intervention_applied",
        OrchestratorCcReceived { .. } => "orchestrator_cc_received",
        TriggerRegistered { .. } => "trigger_registered",
        TriggerFired { .. } => "trigger_fired",
        TriggerDispatched { .. } => "trigger_dispatched",
        TriggerSkipped { .. } => "trigger_skipped",
        TriggerFailed { .. } => "trigger_failed",
        PlatformStarted { .. } => "platform_started",
        PlatformStopping => "platform_stopping",
        SkillStaged { .. } => "skill_staged",
        SkillApproved { .. } => "skill_approved",
        SkillRejected { .. } => "skill_rejected",
        SkillActivated { .. } => "skill_activated",
        NoRoleMatched { .. } => "no_role_matched",
        AgentRequestReview { .. } => "agent_request_review",
        ReviewRequestTimeout { .. } => "review_request_timeout",
        WorkerRegistered { .. } => "worker_registered",
        WorkerHeartbeatStateChanged { .. } => "worker_heartbeat_state_changed",
        WorkerStaleSwept { .. } => "worker_stale_swept",
        AgentMessageQueued { .. } => "agent_message_queued",
        AgentMessageDelivered { .. } => "agent_message_delivered",
        AgentMessageRead { .. } => "agent_message_read",
        AgentMessageFailed { .. } => "agent_message_failed",
        WorkspaceCreated { .. } => "workspace_created",
        WorkspaceMutated { .. } => "workspace_mutated",
        WorkspaceCommitted { .. } => "workspace_committed",
        WorkspaceArchived { .. } => "workspace_archived",
        WorkspaceCollected { .. } => "workspace_collected",
        WorkspaceQuotaExceeded { .. } => "workspace_quota_exceeded",
        WorkspacePromoted { .. } => "workspace_promoted",
        DeliverableProduced { .. } => "deliverable_produced",
        DeliverableAccepted { .. } => "deliverable_accepted",
        DeliverableRejected { .. } => "deliverable_rejected",
        DeliverableExpired { .. } => "deliverable_expired",
        AgentInlineMessagePushed { .. } => "agent_inline_message_pushed",
        Custom { .. } => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::{EventKind, EventMeta};

    fn mk(label: &str) -> Event {
        Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: label.into(),
                payload: serde_json::json!({}),
            },
        }
    }

    #[tokio::test]
    async fn kind_tag_matches_events_layer() {
        // 只抽样几个——存在性保证即可，完整枚举在 events crate 有测试。
        assert_eq!(kind_tag(&EventKind::ThinkingStarted), "thinking_started");
        assert_eq!(
            kind_tag(&EventKind::Custom {
                label: "x".into(),
                payload: serde_json::json!({})
            }),
            "custom"
        );
    }

    #[tokio::test]
    async fn cursor_from_params_parses_uuid() {
        let u = Uuid::new_v4();
        let c = cursor_from_params(Some(&u.to_string()), None).expect("ok");
        assert!(matches!(c, Some(ReplayCursor::FromId(_))));
    }

    #[tokio::test]
    async fn cursor_from_params_parses_time() {
        let c = cursor_from_params(None, Some("2026-04-18T00:00:00Z")).expect("ok");
        assert!(matches!(c, Some(ReplayCursor::FromTime(_))));
    }

    #[tokio::test]
    async fn cursor_from_params_none_gives_none() {
        let c = cursor_from_params(None, None).expect("ok");
        assert!(c.is_none());
    }

    #[tokio::test]
    async fn cursor_from_params_rejects_garbage() {
        assert!(cursor_from_params(Some("not-a-uuid"), None).is_err());
        assert!(cursor_from_params(None, Some("yesterday")).is_err());
    }

    #[tokio::test]
    async fn store_replay_reflects_publishes() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        bus.publish(mk("a")).expect("publish");
        bus.publish(mk("b")).expect("publish");
        // 等 writer flush——无 polling 方法，只能退避。
        tokio::time::sleep(Duration::from_millis(150)).await;

        let mut s = bus.store().replay(ReplayCursor::Beginning);
        let mut got = Vec::new();
        while let Some(item) = s.next().await {
            got.push(item.expect("ok"));
        }
        assert_eq!(got.len(), 2);
    }
}
