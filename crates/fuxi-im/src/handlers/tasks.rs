//! `/api/tasks*`——任务卡片列表 + 单任务事件历史。
//!
//! `list_tasks`（β · #21 实装）：聚合 EventBus 历史 + Fuxi shelf 状态返
//! `{ running, completed }` 两组 TaskCard。具体聚合逻辑见 `crate::tasks_view`。
//!
//! `task_events`（γ 实装）：单任务历史事件回放——HTTP 同步端点，**不 tail**。
//! 实时订阅请走 `WS /api/tasks/{id}/stream`（公理 #3：真实时不轮询）。
//! cursor 缺省 → 该 task 全量历史；带 `?from=<event_id|rfc3339>` → 严格之后。
//! 默认 limit=100，硬上限 1000，防止前端误打分页接口当 dump 工具。

use crate::error::{Error, Result};
use crate::handlers::ws_common::{
    EventHistoryResponse, build_event_stream, parse_cursor, run_ws_loop,
};
use crate::state::AppState;
use crate::tasks_view::{ListTasksResponse, aggregate};
use axum::Json;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use futures_util::StreamExt;
use fuxi_core::task::TaskState;
use fuxi_core::{Event, EventKind, TaskId};
use fuxi_events::ReplayCursor;
use serde::Deserialize;
use tracing::{info, warn};
use uuid::Uuid;

/// 单任务 thread 流的事件白名单 filter（#N6'）。
///
/// 镜像 `handlers::workers::worker_event_visible` 的设计：
/// - 抽出独立 fn 让 HTTP `task_events` + WS `task_conv_ws` + 旧 `task_stream_ws`
///   共用一份语义，避免 filter 漂移
/// - WS 闭包 `'static` 必须 capture by value，把 `task_id` clone 进去
/// - 单测能直接断言 filter 行为，不必起 axum
///
/// 命中 `meta.task == Some(:task_id)` 时再过 EventKind 白名单：
/// - `UserInterventionSent`（用户对该 task 上的 worker 说话）
/// - `AgentResponded`（worker 输出文本；玄女在 task 中也算 member）
/// - `ToolCallStarted` / `ToolCallFinished`
/// - `ThinkingStarted` / `ThinkingFinished`
/// - `TaskStateChanged{Done|Cancelled|Delivering}`（终态 marker）
///
/// 故意**不**收：`AgentSpawning`/`AgentReady`/`AgentDead`/`TaskCreated`/`TaskDispatched`
/// 这些是平台事件不是对话内容；前端不该在任务 thread 渲染。
pub(crate) fn task_thread_visible(ev: &Event, task_id: TaskId) -> bool {
    if ev.meta.task != Some(task_id) {
        return false;
    }
    matches!(
        ev.kind,
        EventKind::UserInterventionSent { .. }
            | EventKind::AgentResponded { .. }
            | EventKind::ToolCallStarted { .. }
            | EventKind::ToolCallFinished { .. }
            | EventKind::ThinkingStarted
            | EventKind::ThinkingFinished
            | EventKind::AgentInlineMessagePushed { .. }
            | EventKind::DeliverableProduced { .. }
            | EventKind::TaskStateChanged {
                to: TaskState::Done | TaskState::Cancelled | TaskState::Delivering,
                ..
            }
    )
}

/// 同 `handlers/conv.rs::parse_task_id`：URL path 段允许裸 UUID 或 `task-<uuid>`。
fn parse_task_id(s: &str) -> std::result::Result<TaskId, String> {
    let trimmed = s.strip_prefix("task-").unwrap_or(s);
    Uuid::parse_str(trimmed)
        .map(TaskId::from)
        .map_err(|e| format!("task id 不是合法的 UUID: {s} ({e})"))
}

#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
pub struct ListTasksQuery {
    /// `root=1` → 只返 root 任务。v1 全部 task 视为 root（无父子关系），
    /// 该参数保留向后兼容；当前忽略。
    pub root: Option<u8>,
}

/// `GET /api/tasks` —— 任务 sheet 数据源（β · #21）。
///
/// 聚合 `EventBus` 全部历史 + `Fuxi` 实时 shelf 状态，返
/// `{ running: TaskCard[], completed: TaskCard[] }`。
/// 详细聚合规则见 `crate::tasks_view::aggregate`。
pub async fn list_tasks(
    State(state): State<AppState>,
    Query(_q): Query<ListTasksQuery>,
) -> Result<Json<ListTasksResponse>> {
    let resp = aggregate(&state.fuxi, state.fuxi.bus()).await?;
    Ok(Json(resp))
}

/// `?from=<cursor>&limit=N` 历史回放查询。
#[derive(Debug, Default, Deserialize)]
pub struct EventsQuery {
    /// 回放起点：事件 UUID 或 RFC3339 时间戳。缺省 = 该 task 历史从头。
    pub from: Option<String>,
    /// 最多返回条数；默认 100，最大 1000。
    pub limit: Option<usize>,
}

/// `GET /api/tasks/:id/events?from=<cursor>&limit=N` —— 单任务事件历史。
///
/// **#N6'** 起加 EventKind 白名单：只下发任务 thread 渲染需要的事件
/// （AgentResponded / ToolCall* / Thinking* / UserInterventionSent /
/// TaskStateChanged 终态）—— 详见 [`task_thread_visible`]。
///
/// 使用 `EventStore::replay` 拉全表流然后按 filter 过滤+分页——
/// 不直接走 `history_for_task` 是因为后者无 cursor 语义。`replay(FromId)` 走
/// rowid 锚点，跨任务统一时间序保持单调。
#[tracing::instrument(skip(state), fields(task_id = %id))]
pub async fn task_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<EventHistoryResponse>> {
    let task_id = parse_task_id(&id).map_err(Error::BadRequest)?;
    let cursor = parse_cursor(q.from.as_deref())?.unwrap_or(ReplayCursor::Beginning);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);

    let store = state.fuxi.bus().store().clone();
    let mut stream = store.replay(cursor);
    let mut out: Vec<Event> = Vec::with_capacity(limit.min(256));

    while let Some(item) = stream.next().await {
        let ev = item?;
        if !task_thread_visible(&ev, task_id) {
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

/// `?from=<cursor>` WS 流式接续 query。
#[derive(Debug, Default, Deserialize)]
pub struct TaskStreamQuery {
    pub from: Option<String>,
}

/// `WS /api/tasks/:id/conv` —— 任务 thread 流式接续（#N6'）。
///
/// 镜像 `WS /api/conv` + `WS /api/workers/:id/conv` 的 pattern，但 filter 走
/// [`task_thread_visible`]（meta.task 命中 + 白名单 EventKind）。
///
/// 与旧 `WS /api/tasks/:id/stream` 区别：
/// - `/conv` 走白名单（前端任务 thread 直接渲染）
/// - `/stream` 早期未上白名单（任务级原始事件流，给观察器用）
/// router.rs 把两条路径都映射到本 handler；行为一致。
#[tracing::instrument(skip(ws, state), fields(task_id = %id))]
pub async fn task_conv_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TaskStreamQuery>,
) -> Result<Response> {
    let task_id = parse_task_id(&id).map_err(|e| {
        warn!(raw = %id, error = %e, "task_id 解析失败");
        Error::BadRequest(e)
    })?;
    let cursor = parse_cursor(q.from.as_deref())?;
    info!(?cursor, %task_id, "ws /api/tasks/{id}/conv accept");

    let bus = state.fuxi.bus().clone();
    let stream = build_event_stream(&bus, cursor);

    let resp = ws.on_upgrade(move |socket| async move {
        run_ws_loop(socket, stream, move |ev| task_thread_visible(ev, task_id)).await;
    });
    Ok(resp)
}

#[cfg(test)]
mod filter_tests {
    //! `task_thread_visible` filter 契约——镜像 `handlers::workers::worker_event_visible`
    //! 的单测格式。让 #N6' 白名单漂移在三 gate 立刻报警，不必等真 server e2e。

    use super::task_thread_visible;
    use chrono::Utc;
    use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskId, task::TaskState};

    fn ev(task: Option<TaskId>, agent: Option<AgentId>, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.task = task;
        meta.agent = agent;
        meta.at = Utc::now();
        Event { meta, kind }
    }

    #[test]
    fn passes_assistant_text_for_target_task() {
        let t = TaskId::new();
        let mine = ev(
            Some(t),
            None,
            EventKind::AgentResponded { text: "hi".into() },
        );
        let other_task = ev(
            Some(TaskId::new()),
            None,
            EventKind::AgentResponded { text: "hi".into() },
        );
        assert!(task_thread_visible(&mine, t));
        assert!(!task_thread_visible(&other_task, t));
    }

    #[test]
    fn passes_tool_call_pair() {
        let t = TaskId::new();
        let started = ev(
            Some(t),
            None,
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command":"ls"}),
            },
        );
        let finished = ev(
            Some(t),
            None,
            EventKind::ToolCallFinished {
                tool: "Bash".into(),
                ok: true,
                output_preview: "ok".into(),
            },
        );
        assert!(task_thread_visible(&started, t));
        assert!(task_thread_visible(&finished, t));
    }

    #[test]
    fn passes_thinking_pair() {
        let t = TaskId::new();
        let s = ev(Some(t), None, EventKind::ThinkingStarted);
        let f = ev(Some(t), None, EventKind::ThinkingFinished);
        assert!(task_thread_visible(&s, t));
        assert!(task_thread_visible(&f, t));
    }

    #[test]
    fn passes_user_intervention_when_meta_task_matches() {
        // 任务 thread 的 UserInterventionSent 按 meta.task 命中即可——和 worker
        // 私聊页按 target 命中是不同维度。任务 thread 内"用户对该任务说了话"
        // 这件事按 meta.task 区分。
        let t = TaskId::new();
        let intervene = ev(
            Some(t),
            None,
            EventKind::UserInterventionSent {
                target: AgentId::new(),
                mode: "append".into(),
                text: "x".into(),
                mentions: Vec::new(),
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
            },
        );
        assert!(task_thread_visible(&intervene, t));
        let other = ev(
            Some(TaskId::new()),
            None,
            EventKind::UserInterventionSent {
                target: AgentId::new(),
                mode: "append".into(),
                text: "x".into(),
                mentions: Vec::new(),
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
            },
        );
        assert!(!task_thread_visible(&other, t));
    }

    #[test]
    fn passes_task_terminal_state() {
        let t = TaskId::new();
        let done = ev(
            Some(t),
            None,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        );
        let cancelled = ev(
            Some(t),
            None,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Cancelled,
            },
        );
        let delivering = ev(
            Some(t),
            None,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Delivering,
            },
        );
        let mid_state = ev(
            Some(t),
            None,
            EventKind::TaskStateChanged {
                from: TaskState::Ready,
                to: TaskState::InProgress,
            },
        );
        assert!(task_thread_visible(&done, t));
        assert!(task_thread_visible(&cancelled, t));
        assert!(task_thread_visible(&delivering, t));
        assert!(
            !task_thread_visible(&mid_state, t),
            "中间态不算 task_completed 不入 thread"
        );
    }

    #[test]
    fn drops_non_whitelisted_kinds() {
        let t = TaskId::new();
        // AgentSpawning / TaskCreated / TaskDispatched / Custom 都是平台事件
        // 而非对话内容，不进 thread
        let spawning = ev(
            Some(t),
            None,
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "claude-code".into(),
            },
        );
        assert!(!task_thread_visible(&spawning, t));
        let created = ev(
            Some(t),
            None,
            EventKind::TaskCreated {
                title: "x".into(),
                description: "x".into(),
            },
        );
        assert!(!task_thread_visible(&created, t));
        let dispatched = ev(
            Some(t),
            None,
            EventKind::TaskDispatched { to: AgentId::new() },
        );
        assert!(!task_thread_visible(&dispatched, t));
        let custom = ev(
            Some(t),
            None,
            EventKind::Custom {
                label: "x".into(),
                payload: serde_json::json!({}),
            },
        );
        assert!(!task_thread_visible(&custom, t));
    }

    #[test]
    fn drops_when_meta_task_missing() {
        let t = TaskId::new();
        let no_task = ev(None, None, EventKind::AgentResponded { text: "x".into() });
        assert!(!task_thread_visible(&no_task, t));
    }
}

#[cfg(test)]
mod tests {
    //! `list_tasks` HTTP 端到端：EventBus 灌真实事件 → 调 handler → 验返回结构。

    use crate::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get as axum_get;
    use chrono::{Duration as ChronoDuration, Utc};
    use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskId, task::TaskState};
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

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

    async fn build_app() -> (
        tempfile::TempDir,
        Router,
        EventBus,
        Arc<Fuxi>,
        AgentId, // xuannv id
    ) {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let (dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus.clone(), ws));
        let xuannv = AgentId::new();
        fuxi.set_xuannv(xuannv).await;
        let state = AppState::new(fuxi.clone());
        let app = Router::new()
            .route("/api/tasks", axum_get(super::list_tasks))
            .with_state(state);
        (dir, app, bus, fuxi, xuannv)
    }

    fn make_event(
        task: TaskId,
        agent: Option<AgentId>,
        at: chrono::DateTime<Utc>,
        kind: EventKind,
    ) -> Event {
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        meta.agent = agent;
        meta.at = at;
        Event { meta, kind }
    }

    async fn fetch(app: Router) -> serde_json::Value {
        let req = Request::builder()
            .uri("/api/tasks")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn empty_history_returns_empty_groups() {
        let (_dir, app, _bus, _fuxi, _xn) = build_app().await;
        let v = fetch(app).await;
        assert_eq!(v["running"].as_array().unwrap().len(), 0);
        assert_eq!(v["completed"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn running_and_completed_groups_split() {
        let (_dir, app, bus, _fuxi, xn) = build_app().await;
        let t0 = Utc::now();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();
        let task_running = TaskId::new();
        let task_done = TaskId::new();

        // running task：派给 agent_a 但没 Done
        bus.publish(make_event(
            task_running,
            None,
            t0,
            EventKind::TaskCreated {
                title: "修 ERP-1066".into(),
                description: "复现".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task_running,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent_a },
        ))
        .unwrap();

        // completed task：派给 agent_b + TaskStateChanged → Done
        bus.publish(make_event(
            task_done,
            None,
            t0 - ChronoDuration::seconds(60),
            EventKind::TaskCreated {
                title: "改 PR 标题".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task_done,
            None,
            t0 - ChronoDuration::seconds(59),
            EventKind::TaskDispatched { to: agent_b },
        ))
        .unwrap();
        bus.publish(make_event(
            task_done,
            None,
            t0 - ChronoDuration::seconds(50),
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        ))
        .unwrap();

        // 玄女自己派给玄女自己的 user-turn —— 应被过滤
        let task_user_turn = TaskId::new();
        bus.publish(make_event(
            task_user_turn,
            None,
            t0,
            EventKind::TaskCreated {
                title: "user-turn".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task_user_turn,
            None,
            t0,
            EventKind::TaskDispatched { to: xn },
        ))
        .unwrap();

        // 等 EventStore writer flush
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        let v = fetch(app).await;
        let running = v["running"].as_array().unwrap();
        let completed = v["completed"].as_array().unwrap();
        assert_eq!(running.len(), 1, "1 running task；user-turn 应被过滤");
        assert_eq!(completed.len(), 1);
        assert_eq!(running[0]["title"], "修 ERP-1066");
        assert_eq!(running[0]["status"], "running");
        assert_eq!(completed[0]["title"], "改 PR 标题");
        assert_eq!(completed[0]["status"], "completed");
    }

    #[tokio::test]
    async fn member_activity_picked_from_recent_tool_call() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();

        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        // 最近 tool call → activity 用它
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(2),
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command": "cargo test --lib"}),
            },
        ))
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let v = fetch(app).await;
        let running = v["running"].as_array().unwrap();
        assert_eq!(running.len(), 1);
        let members = running[0]["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        let activity = members[0]["activity"].as_str().unwrap_or("");
        assert!(
            activity.contains("Bash") && activity.contains("cargo test"),
            "activity 应展示 tool call；得到 {activity:?}"
        );
    }

    #[tokio::test]
    async fn shape_matches_contract() {
        // 单 running task → 验 JSON 字段名全到位（前端契约）
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();
        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "X".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        let v = fetch(app).await;
        assert!(v.get("running").is_some());
        assert!(v.get("completed").is_some());
        let card = &v["running"][0];
        for k in [
            "id",
            "title",
            "status",
            "created_at",
            "last_active_at",
            "duration_ms",
            "members",
        ] {
            assert!(card.get(k).is_some(), "TaskCard 缺字段 {k}");
        }
        let m = &card["members"][0];
        // 重设计 #25 字段加齐：last_tool_call + description
        // β · #55 加齐：node_id（v1 = "home"）
        for k in [
            "agent_id",
            "role",
            "role_display",
            "activity",
            "tokens",
            "status",
            "last_tool_call",
            "description",
            "node_id",
            "phase",
            "tool_use_count",
            "last_activity",
            "recent_activities",
            "summary",
        ] {
            assert!(m.get(k).is_some(), "MemberCard 缺字段 {k}");
        }
        // β · #55 v1 默认值断言
        assert_eq!(m["node_id"], "home", "v1 本地 spawn 默认归 home");
    }

    /// 重设计 #25：title=="user-turn" 的 task **不**进 list_tasks 响应。
    /// 用户实测撞过 9 条 user-turn 充满 sheet。
    #[tokio::test]
    async fn user_turn_tasks_filtered_out() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let t0 = Utc::now();
        let agent_a = AgentId::new();
        let user_turn_id = TaskId::new();
        let real_id = TaskId::new();

        // user-turn task：派给真 agent 也不该进列表（title 字面 == "user-turn"）
        bus.publish(make_event(
            user_turn_id,
            None,
            t0,
            EventKind::TaskCreated {
                title: "user-turn".into(),
                description: "用户对玄女说话".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            user_turn_id,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent_a },
        ))
        .unwrap();

        // 真 task：title 不是 user-turn，正常进列表
        bus.publish(make_event(
            real_id,
            None,
            t0 + ChronoDuration::seconds(2),
            EventKind::TaskCreated {
                title: "修 ERP-1066".into(),
                description: "复现 + 修".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            real_id,
            None,
            t0 + ChronoDuration::seconds(3),
            EventKind::TaskDispatched { to: agent_a },
        ))
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let v = fetch(app).await;
        let running = v["running"].as_array().unwrap();
        assert_eq!(running.len(), 1, "只有真 task 入列；user-turn 应过滤");
        assert_eq!(running[0]["title"], "修 ERP-1066");
    }

    /// 重设计 #25：member.last_tool_call 用 spec 严格 "·" 分隔。
    #[tokio::test]
    async fn member_last_tool_call_uses_dot_separator() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();
        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "复现 + 修".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(2),
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command": "grep server/api/v1.go"}),
            },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let v = fetch(app).await;
        let m = &v["running"][0]["members"][0];
        let ltc = m["last_tool_call"].as_str().unwrap_or("");
        assert!(
            ltc == "Bash · grep server/api/v1.go",
            "spec 要求严格 `Bash · ...` 格式，得到 {ltc:?}"
        );
        // description 也应进 member（task 派活时附的）
        assert_eq!(m["description"].as_str(), Some("复现 + 修"));
    }

    /// 进度快照：/api/tasks member 从真实事件流物化 teammate-style progress 字段。
    #[tokio::test]
    async fn member_progress_snapshot_materialized_from_events() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();
        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "补进度快照".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(2),
            EventKind::ThinkingStarted,
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(3),
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command": "cargo test -p fuxi-im"}),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(4),
            EventKind::ToolCallStarted {
                tool: "Read".into(),
                args: serde_json::json!({"file_path": "crates/fuxi-im/src/tasks_view.rs"}),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(5),
            EventKind::AgentResponded {
                text: "快照字段已接入".into(),
            },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        let v = fetch(app).await;
        let m = &v["running"][0]["members"][0];
        assert_eq!(m["phase"], "responded");
        assert_eq!(m["tool_use_count"], 2);
        assert_eq!(m["summary"], "快照字段已接入");
        assert_eq!(m["last_activity"], "快照字段已接入");
        let recent = m["recent_activities"].as_array().unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], "Bash · cargo test -p fuxi-im");
        assert_eq!(recent[1], "Read · crates/fuxi-im/src/tasks_view.rs");
    }

    /// 重设计 #25：没发过工具调用的 member，last_tool_call=null（而非 "" 或 fallback text）。
    #[tokio::test]
    async fn member_last_tool_call_null_when_no_tool_yet() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();
        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        // agent 只发了 text，没工具调用
        bus.publish(make_event(
            task,
            Some(agent),
            t0 + ChronoDuration::seconds(2),
            EventKind::AgentResponded {
                text: "好的".into(),
            },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let v = fetch(app).await;
        let m = &v["running"][0]["members"][0];
        assert!(m["last_tool_call"].is_null(), "无工具调用应返 null：{m}");
        // description 空字符串 → null
        assert!(m["description"].is_null(), "空 description 应转 null");
    }

    /// 重设计 #25：member.status 严格三态。Dead agent 应返 "dead"（之前是 "idle"）。
    #[tokio::test]
    async fn member_status_dead_when_agent_dead() {
        // 测试通过 fuxi.status_of 返 None 路径走 dead——shelf 没注册的 agent
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let unregistered = AgentId::new();
        let t0 = Utc::now();
        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: unregistered },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let v = fetch(app).await;
        let status = v["running"][0]["members"][0]["status"]
            .as_str()
            .unwrap_or("");
        // shelf 不在该 agent + 没 thinking → "dead"（spec 要求三态明确，不再降级 "idle"）
        assert_eq!(
            status, "dead",
            "shelf 不在的 agent 应返 dead，得到 {status}"
        );
    }

    /// #51 修：worker GC / shutdown 后 shelf 没了，role 应从 EventBus 历史
    /// `AgentSpawning` 事件兜底拿，而不是 fallback 到字面 "unknown"。
    #[tokio::test]
    async fn role_display_falls_back_to_agent_spawning_event_when_shelf_missing() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new(); // 故意没插 shelf——模拟 GC / shutdown 后历史里仍有
        let t0 = Utc::now();

        // 历史第一条：AgentSpawning 记录 role
        bus.publish(make_event(
            task,
            Some(agent),
            t0,
            EventKind::AgentSpawning {
                role: "luban".into(),
                cli: "cc".into(),
            },
        ))
        .unwrap();
        // task 派给该 agent
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(2),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        let v = fetch(app).await;
        let m = &v["running"][0]["members"][0];
        // role 从 AgentSpawning 兜底 → "luban" → role_display "鲁班"
        assert_eq!(
            m["role"], "luban",
            "shelf 缺失时应从历史 AgentSpawning 兜底"
        );
        assert_eq!(m["role_display"], "鲁班", "role_display 不应是 'unknown'");
    }

    /// #51（旧）+ user 实测（2026-05-04）超越：task 已 Done 时 member.status
    /// 反映 live shelf——shelf None/Dead → "dead"（worker 已 GC/挂掉），
    /// shelf Idle/Busy → "idle"（worker 还活着，可召回）。
    /// 用户视角："鲁班还在我能再派活吗" 的答案就是这个字段。
    #[tokio::test]
    async fn member_status_after_task_done_reflects_shelf_liveness() {
        let (_dir, app, bus, fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();

        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(5),
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        ))
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        // 子用例 a) shelf 不存在该 agent（GC 走）→ "dead"
        let _ = fuxi.clone(); // 不 insert agent
        let v = fetch(app.clone()).await;
        let card = &v["completed"][0];
        assert_eq!(card["status"], "completed", "task 应入 completed 桶");
        assert_eq!(
            card["members"][0]["status"], "dead",
            "task done + worker 不在 shelf → dead（GC 后状态）"
        );
    }

    /// Bug 1 回归：task 事件流里**缺失** `TaskCreated`（旧库被截断 / 仅留下后段
    /// ToolCall/AgentResponded 等）时，`created_at` 不应回落到 `Utc::now()`，
    /// 否则会出现 `created_at > last_active_at` 的「未来创建」脏数据。
    /// 期望：取该 task 最早事件时刻作 `created_at` 兜底，保 `created <= last_active`。
    #[tokio::test]
    async fn created_at_falls_back_to_first_event_when_task_created_missing() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        // 故意把所有 at 设为「过去」——若 fallback 用 Utc::now() 会比这些都大。
        let past = Utc::now() - ChronoDuration::days(3);

        // 没 TaskCreated；只有 ToolCall + AgentResponded（都带 meta.task + meta.agent）
        bus.publish(make_event(
            task,
            Some(agent),
            past,
            EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command": "cargo test"}),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            Some(agent),
            past + ChronoDuration::seconds(10),
            EventKind::AgentResponded {
                text: "完成".into(),
            },
        ))
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let v = fetch(app).await;
        let running = v["running"].as_array().unwrap();
        assert_eq!(running.len(), 1, "task 应入列（有真 member 累积）");
        let card = &running[0];
        let created: chrono::DateTime<Utc> = card["created_at"]
            .as_str()
            .unwrap()
            .parse()
            .expect("rfc3339");
        let last_active: chrono::DateTime<Utc> = card["last_active_at"]
            .as_str()
            .unwrap()
            .parse()
            .expect("rfc3339");
        assert!(
            created <= last_active,
            "created_at ({created}) 不应晚于 last_active_at ({last_active})"
        );
        // 兜底应是最早事件时刻（3 天前），不是 Utc::now()。
        assert!(
            created < Utc::now() - ChronoDuration::hours(1),
            "created_at 不应回落到当前时刻 ({created})；应取最早事件时刻"
        );
    }

    /// Bug 3 回归：老 task 的 `AgentSpawning` 事件被 `recent(2000)` 窗口截断后，
    /// role 兜底**仍**应该命中——证明聚合不依赖最近 N 条窗口，而是按 kind 全表扫。
    /// 用 2100 条噪声 Custom 事件挤掉 AgentSpawning 出 recent(2000)。
    #[tokio::test]
    async fn role_falls_back_even_when_agent_spawning_outside_recent_2000() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let store = bus.store();
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now() - ChronoDuration::days(4);

        // 用 store.append 直接同步落库（绕开 broadcast 异步路径）——大批量灌也快。
        store
            .append(&make_event(
                task,
                Some(agent),
                t0,
                EventKind::AgentSpawning {
                    role: "luban".into(),
                    cli: "cc".into(),
                },
            ))
            .await
            .expect("append spawning");
        store
            .append(&make_event(
                task,
                None,
                t0 + ChronoDuration::seconds(1),
                EventKind::TaskCreated {
                    title: "T".into(),
                    description: "".into(),
                },
            ))
            .await
            .expect("append created");
        store
            .append(&make_event(
                task,
                None,
                t0 + ChronoDuration::seconds(2),
                EventKind::TaskDispatched { to: agent },
            ))
            .await
            .expect("append dispatched");

        // 灌 2100 条噪声 Custom 事件——使 AgentSpawning 落在 rowid 倒序的 2000 条之外。
        let noise_task = TaskId::new();
        for i in 0..2100u32 {
            store
                .append(&make_event(
                    noise_task,
                    None,
                    t0 + ChronoDuration::seconds(10 + i as i64),
                    EventKind::Custom {
                        label: format!("noise-{i}"),
                        payload: serde_json::json!({}),
                    },
                ))
                .await
                .expect("append noise");
        }

        let v = fetch(app).await;
        let running = v["running"].as_array().unwrap();
        let card = running
            .iter()
            .find(|c| c["id"].as_str().unwrap_or("").contains(&task.to_string()))
            .expect("老 task 应入列");
        let m = &card["members"][0];
        assert_eq!(
            m["role"], "luban",
            "AgentSpawning 即便不在 recent(2000) 窗口内，role 兜底仍应命中"
        );
        assert_eq!(m["role_display"], "鲁班", "role_display 不应是 'unknown'");
    }

    /// #51 边界：task 仍 running 时 member.status 仍读 live shelf（行为不变）。
    /// 确认 fix 不破坏 running task 的实时 status。
    #[tokio::test]
    async fn member_status_reads_live_shelf_when_task_running() {
        let (_dir, app, bus, _fuxi, _xn) = build_app().await;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();

        bus.publish(make_event(
            task,
            None,
            t0,
            EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        ))
        .unwrap();
        bus.publish(make_event(
            task,
            None,
            t0 + ChronoDuration::seconds(1),
            EventKind::TaskDispatched { to: agent },
        ))
        .unwrap();
        // 没发 TaskStateChanged → task 仍 running
        tokio::time::sleep(std::time::Duration::from_millis(220)).await;

        let v = fetch(app).await;
        let card = &v["running"][0];
        assert_eq!(card["status"], "running");
        // shelf 没该 agent → live status 是 "dead"（前 fix 路径不被覆盖）
        assert_eq!(card["members"][0]["status"], "dead");
    }
}
