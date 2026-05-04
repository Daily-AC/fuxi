//! `/api/workers/:agent_id/{events,conv}` —— 单门客私聊页的事件流（Task #27 / 重设计 #N5）。
//!
//! 镜像 `/api/conv` 但按 `meta.agent == :agent_id` 过滤，再叠加事件 kind 白名单。
//! ε 私聊页（橙色识别 modal）拉历史走 `events` HTTP，接续走 `conv` WS。
//!
//! ## filter 规则
//!
//! `meta.agent == :agent_id` 命中**且**事件 kind 在白名单：
//! - `AgentResponded`（spec 中"AssistantText"，cc 一次性 final 文本）
//! - `ToolCallStarted` / `ToolCallFinished`（spec "ToolStarted/Finished"）
//! - `ThinkingStarted` / `ThinkingFinished`（spec "ThinkingDone"）
//! - `TaskStateChanged { to: Done | Cancelled | Delivering }`（spec "task_completed"）
//! - `UserPrompted`（玄女→门客 dispatch 自带的 prompt）
//!
//! **特殊**：`UserInterventionSent { target == :agent_id }` 即使 `meta.agent` 不指向
//! 该门客也要透出——用户对门客说话的场景里 publisher 把 meta.agent 设成 xuannv（抄送），
//! `target` 字段才是真正的"消息要给谁"。这条单独走 target 判定，其余都走 meta.agent。
//!
//! `agent_idle` spec 里有但 EventKind 没有该变体——shelf 状态不通过 EventBus 发，
//! 前端可由 `TaskStateChanged → Done` 推"该 agent 这次活完了"。本端点暂不合成
//! 假事件（避免污染事件流真实性）。

use crate::error::{Error, Result};
use crate::handlers::ws_common::{
    EventHistoryResponse, build_event_stream, parse_cursor, run_ws_loop,
};
use crate::state::AppState;
use axum::Json;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use futures_util::StreamExt;
use fuxi_core::task::TaskState;
use fuxi_core::{AgentId, Event, EventKind};
use fuxi_events::ReplayCursor;
use serde::Deserialize;
use tracing::{info, warn};
use uuid::Uuid;

/// 解析 `:agent_id` path 段——接受裸 UUID 或 `agent-<uuid>` Display 形式。
/// 跟 `parse_task_id` 同款约定（`fuxi_core::id::AgentId` 的 Display 是 `agent-<uuid>`）。
fn parse_agent_id(s: &str) -> std::result::Result<AgentId, String> {
    let trimmed = s.strip_prefix("agent-").unwrap_or(s);
    Uuid::parse_str(trimmed)
        .map(AgentId::from)
        .map_err(|e| format!("agent id 不是合法的 UUID: {s} ({e})"))
}

/// 事件是否属于该门客私聊页流——filter 闭包共享给 history HTTP + WS。
///
/// 抽出独立 fn 因为：
/// 1. WS 闭包要 `'static`，必须 capture by value，把 `agent_id` clone 进去
/// 2. HTTP handler 也要同款语义——一个真相源避免漂移
/// 3. 单测能直接断言 filter 行为，不必起 axum
pub(crate) fn worker_event_visible(ev: &Event, agent_id: AgentId) -> bool {
    // 特殊：UserInterventionSent 看 target 而非 meta.agent
    if let EventKind::UserInterventionSent { target, .. } = &ev.kind {
        return *target == agent_id;
    }
    // 其它事件先过 meta.agent
    if ev.meta.agent != Some(agent_id) {
        return false;
    }
    matches!(
        ev.kind,
        EventKind::AgentResponded { .. }
            | EventKind::ToolCallStarted { .. }
            | EventKind::ToolCallFinished { .. }
            | EventKind::ThinkingStarted
            | EventKind::ThinkingFinished
            | EventKind::UserPrompted { .. }
            | EventKind::AgentInlineMessagePushed { .. }
            | EventKind::DeliverableProduced { .. }
            | EventKind::TaskStateChanged {
                to: TaskState::Done | TaskState::Cancelled | TaskState::Delivering,
                ..
            }
    )
}

/// `?from=<cursor>&limit=N` 历史回放查询。
#[derive(Debug, Default, Deserialize)]
pub struct WorkerEventsQuery {
    pub from: Option<String>,
    /// 默认 100，硬上限 1000。
    pub limit: Option<usize>,
}

/// `?from=<cursor>` WS 流式接续查询。
#[derive(Debug, Default, Deserialize)]
pub struct WorkerStreamQuery {
    pub from: Option<String>,
}

/// `GET /api/workers/:agent_id/events?from=<cursor>` —— 私聊页历史拉取。
///
/// 走 `EventStore::replay` + filter；和 `task_events` 同模式。
#[tracing::instrument(skip(state), fields(agent_id = %id))]
pub async fn worker_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WorkerEventsQuery>,
) -> Result<Json<EventHistoryResponse>> {
    let agent_id = parse_agent_id(&id).map_err(|e| {
        warn!(raw = %id, error = %e, "agent_id 解析失败");
        Error::BadRequest(e)
    })?;
    let cursor = parse_cursor(q.from.as_deref())?.unwrap_or(ReplayCursor::Beginning);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);

    let store = state.fuxi.bus().store().clone();
    let mut stream = store.replay(cursor);
    let mut out: Vec<Event> = Vec::with_capacity(limit.min(256));

    while let Some(item) = stream.next().await {
        let ev = item?;
        if !worker_event_visible(&ev, agent_id) {
            continue;
        }
        out.push(ev);
        if out.len() >= limit {
            break;
        }
    }
    Ok(Json(EventHistoryResponse {
        events: out,
        next_cursor: None,
    }))
}

/// `WS /api/workers/:agent_id/conv` —— 私聊页流式接续。
///
/// 镜像 `/api/conv` 但 filter 走 `worker_event_visible`。
#[tracing::instrument(skip(ws, state), fields(agent_id = %id))]
pub async fn worker_conv_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WorkerStreamQuery>,
) -> Result<Response> {
    let agent_id = parse_agent_id(&id).map_err(|e| {
        warn!(raw = %id, error = %e, "agent_id 解析失败");
        Error::BadRequest(e)
    })?;
    let cursor = parse_cursor(q.from.as_deref())?;
    info!(?cursor, %agent_id, "ws /api/workers/{id}/conv accept");

    let bus = state.fuxi.bus().clone();
    let stream = build_event_stream(&bus, cursor);

    let resp = ws.on_upgrade(move |socket| async move {
        run_ws_loop(socket, stream, move |ev| worker_event_visible(ev, agent_id)).await;
    });
    Ok(resp)
}

#[cfg(test)]
mod tests {
    //! 单测覆盖 `worker_event_visible` 的 filter 契约 + `worker_events` HTTP 端到端。
    //! WS handler 由 `tests/ws_stream.rs` 同款 e2e 模板覆盖（如未加，借 #N3 实装时
    //! 顺手补；此模块的 filter 单测已锁住核心契约）。

    use super::*;
    use crate::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get as axum_get;
    use chrono::Utc;
    use fuxi_core::{Event, EventKind, EventMeta, TaskId};
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn ev(agent: Option<AgentId>, task: Option<TaskId>, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = agent;
        meta.task = task;
        meta.at = Utc::now();
        Event { meta, kind }
    }

    #[test]
    fn filter_passes_assistant_text_for_target_agent() {
        let me = AgentId::new();
        let other = AgentId::new();
        let mine = ev(
            Some(me),
            None,
            EventKind::AgentResponded { text: "hi".into() },
        );
        let theirs = ev(
            Some(other),
            None,
            EventKind::AgentResponded { text: "hi".into() },
        );
        assert!(worker_event_visible(&mine, me));
        assert!(!worker_event_visible(&theirs, me));
    }

    #[test]
    fn filter_passes_tool_call_pair() {
        let me = AgentId::new();
        let started = ev(
            Some(me),
            None,
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command":"ls"}),
            },
        );
        let finished = ev(
            Some(me),
            None,
            EventKind::ToolCallFinished {
                tool: "Bash".into(),
                ok: true,
                output_preview: "ok".into(),
            },
        );
        assert!(worker_event_visible(&started, me));
        assert!(worker_event_visible(&finished, me));
    }

    #[test]
    fn filter_passes_thinking_pair() {
        let me = AgentId::new();
        let s = ev(Some(me), None, EventKind::ThinkingStarted);
        let f = ev(Some(me), None, EventKind::ThinkingFinished);
        assert!(worker_event_visible(&s, me));
        assert!(worker_event_visible(&f, me));
    }

    #[test]
    fn filter_passes_user_intervention_by_target_not_meta_agent() {
        // 用户对鲁班说话——publisher 在抄送时通常把 meta.agent 设成玄女，
        // 但 target = 鲁班；私聊页要看到这条。
        let luban = AgentId::new();
        let xuannv = AgentId::new();
        let intervene = ev(
            Some(xuannv), // meta.agent 是抄送目标（玄女），不是鲁班
            None,
            EventKind::UserInterventionSent {
                target: luban,
                mode: "append".into(),
                text: "你查下 ERP-1066".into(),
                mentions: vec![luban],
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
            },
        );
        assert!(
            worker_event_visible(&intervene, luban),
            "UserInterventionSent 应按 target 匹配"
        );
        // 别的门客的 intervene 不该出现在鲁班私聊页
        let mo = AgentId::new();
        let other = ev(
            Some(xuannv),
            None,
            EventKind::UserInterventionSent {
                target: mo,
                mode: "append".into(),
                text: "x".into(),
                mentions: vec![mo],
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
            },
        );
        assert!(!worker_event_visible(&other, luban));
    }

    #[test]
    fn filter_passes_task_terminal_state() {
        let me = AgentId::new();
        let t = TaskId::new();
        let done = ev(
            Some(me),
            Some(t),
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        );
        let cancelled = ev(
            Some(me),
            Some(t),
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Cancelled,
            },
        );
        let delivering = ev(
            Some(me),
            Some(t),
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Delivering,
            },
        );
        let mid_state = ev(
            Some(me),
            Some(t),
            EventKind::TaskStateChanged {
                from: TaskState::Ready,
                to: TaskState::InProgress,
            },
        );
        assert!(worker_event_visible(&done, me));
        assert!(worker_event_visible(&cancelled, me));
        assert!(worker_event_visible(&delivering, me));
        assert!(
            !worker_event_visible(&mid_state, me),
            "中间态不算 task_completed 不该入私聊页"
        );
    }

    #[test]
    fn filter_drops_non_whitelisted_kinds() {
        let me = AgentId::new();
        // AgentSpawning / TaskCreated / TaskDispatched 这些都不在白名单
        let spawning = ev(
            Some(me),
            None,
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "claude-code".into(),
            },
        );
        assert!(!worker_event_visible(&spawning, me));
        let created = ev(
            Some(me),
            None,
            EventKind::TaskCreated {
                title: "x".into(),
                description: "x".into(),
            },
        );
        assert!(!worker_event_visible(&created, me));
        let dispatched = ev(
            Some(me),
            None,
            EventKind::TaskDispatched { to: AgentId::new() },
        );
        assert!(!worker_event_visible(&dispatched, me));
    }

    #[test]
    fn filter_drops_when_meta_agent_missing() {
        // 没设 meta.agent 的事件（早期模式）—— filter 直接拒（除 UserInterventionSent
        // 这条按 target 走的特殊路径）
        let me = AgentId::new();
        let no_agent = ev(None, None, EventKind::AgentResponded { text: "x".into() });
        assert!(!worker_event_visible(&no_agent, me));
    }

    // ─── HTTP 端到端：worker_events ────────────────────────────────

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        run_git(path, &["init", "-q", "-b", "main"]).await;
        tokio::fs::write(path.join("README.md"), "seed")
            .await
            .unwrap();
        run_git(path, &["add", "-A"]).await;
        run_git(
            path,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        )
        .await;
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path.to_path_buf()));
        (dir, ws)
    }

    async fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    async fn build_app() -> (tempfile::TempDir, Router, EventBus) {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus.clone(), ws));
        let state = AppState::new(fuxi);
        let app = Router::new()
            .route("/api/workers/{id}/events", axum_get(super::worker_events))
            .with_state(state);
        (dir, app, bus)
    }

    #[tokio::test]
    async fn worker_events_returns_only_target_agent_history() {
        let (_dir, app, bus) = build_app().await;
        let me = AgentId::new();
        let other = AgentId::new();

        // 我的：assistant text + tool call
        bus.publish(ev(
            Some(me),
            None,
            EventKind::AgentResponded {
                text: "我说话".into(),
            },
        ))
        .unwrap();
        bus.publish(ev(
            Some(me),
            None,
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command":"ls"}),
            },
        ))
        .unwrap();
        // 别人的：不该入
        bus.publish(ev(
            Some(other),
            None,
            EventKind::AgentResponded {
                text: "别人说话".into(),
            },
        ))
        .unwrap();
        // 给我的 intervene（meta.agent=玄女，target=me）
        let xuannv = AgentId::new();
        bus.publish(ev(
            Some(xuannv),
            None,
            EventKind::UserInterventionSent {
                target: me,
                mode: "append".into(),
                text: "干活".into(),
                mentions: vec![me],
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
            },
        ))
        .unwrap();
        // 非白名单：spawning 给我 — 该被过滤
        bus.publish(ev(
            Some(me),
            None,
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "claude-code".into(),
            },
        ))
        .unwrap();

        // 等 EventStore writer flush
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workers/{me}/events"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        // P0.D 修后 wire shape = `{events, next_cursor}`（对齐前端 EventHistoryResponse）
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let evs: Vec<Event> = serde_json::from_value(body["events"].clone()).unwrap();
        assert!(
            body["next_cursor"].is_null(),
            "next_cursor 当前应为 null（无服务端分页）"
        );
        assert_eq!(
            evs.len(),
            3,
            "应只有 3 条（assistant + tool + intervene），spawning 和别人发言被过滤"
        );
    }

    #[tokio::test]
    async fn worker_events_bad_agent_id_returns_400() {
        let (_dir, app, _bus) = build_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/workers/not-a-uuid/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn worker_events_accepts_agent_dash_prefix() {
        // AgentId Display 是 `agent-<uuid>`——前端可能直接传该形式
        let (_dir, app, _bus) = build_app().await;
        let me = AgentId::new();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/workers/{me}/events"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
