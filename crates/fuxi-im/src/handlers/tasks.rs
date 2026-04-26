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
use crate::handlers::ws_common::parse_cursor;
use crate::state::AppState;
use crate::tasks_view::{ListTasksResponse, aggregate};
use axum::Json;
use axum::extract::{Path, Query, State};
use futures_util::StreamExt;
use fuxi_core::{Event, TaskId};
use fuxi_events::ReplayCursor;
use serde::Deserialize;
use uuid::Uuid;

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
/// 使用 `EventStore::replay` 拉全表流然后按 `meta.task == :id` 过滤+分页——
/// 不直接走 `history_for_task` 是因为后者无 cursor 语义。`replay(FromId)` 走
/// rowid 锚点，跨任务统一时间序保持单调。
#[tracing::instrument(skip(state), fields(task_id = %id))]
pub async fn task_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<Vec<Event>>> {
    let task_id = parse_task_id(&id).map_err(Error::BadRequest)?;
    let cursor = parse_cursor(q.from.as_deref())?.unwrap_or(ReplayCursor::Beginning);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);

    let store = state.fuxi.bus().store().clone();
    let mut stream = store.replay(cursor);
    let mut out: Vec<Event> = Vec::with_capacity(limit.min(256));

    while let Some(item) = stream.next().await {
        let ev = item?;
        if ev.meta.task != Some(task_id) {
            continue;
        }
        out.push(ev);
        if out.len() >= limit {
            break;
        }
    }
    Ok(Json(out))
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
        for k in [
            "agent_id",
            "role",
            "role_display",
            "activity",
            "tokens",
            "status",
        ] {
            assert!(m.get(k).is_some(), "MemberCard 缺字段 {k}");
        }
    }
}
