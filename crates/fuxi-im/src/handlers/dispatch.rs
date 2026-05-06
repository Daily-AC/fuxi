//! `POST /api/dispatch` —— 强制开新 root task（与 intervene degrade 路径并列）。
//!
//! 用户在 PWA 用"派活"入口（区别于"跟玄女说"）时调用——明确表达"这是一条新任务，
//! 不要追加到当前对话"。
//!
//! 流程：
//! 1. 取 `state.fuxi.xuannv_id().await`
//! 2. 调 `Fuxi::dispatch(xuannv, Task::new(title, description))`
//! 3. 返 200 `{ "task_id": "<uuid>" }` 让前端能拿到任务 id 直接跳到 task chat 视图

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use fuxi_core::ProjectId;
use fuxi_core::task::Task;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct DispatchBody {
    /// 任务标题（短）—— 列表卡片显示。
    pub title: String,
    /// 任务详情（长）—— 玄女 / 门客读到的具体诉求。
    pub description: String,
    /// v2 跨节点 sandbox：把 task 关联到 project。dispatch 看到时按
    /// project.host_nodes auto-pin 到最闲节点 + worker 端 spawn 进对应 sandbox。
    /// `None` = 不绑项目（保留 v1 行为，cc 跑在 dispatch cwd）。
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DispatchResponse {
    /// 创建的 task_id（uuid）—— 前端可立即跳到 `#/task/<id>`。
    pub task_id: String,
}

pub async fn dispatch(
    State(state): State<AppState>,
    Json(body): Json<DispatchBody>,
) -> Result<Json<DispatchResponse>> {
    let title = body.title.trim();
    let description = body.description.trim();
    if title.is_empty() {
        return Err(Error::BadRequest("title 不能为空".into()));
    }
    if description.is_empty() {
        return Err(Error::BadRequest("description 不能为空".into()));
    }

    let xuannv =
        state.fuxi.xuannv_id().await.ok_or_else(|| {
            Error::Unavailable("玄女尚未就绪——请稍后重试或检查 daemon 启动".into())
        })?;

    let mut task = Task::new(title, description);
    if let Some(slug) = body
        .project
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let pid = ProjectId::new(slug.to_string())
            .map_err(|e| Error::BadRequest(format!("非法 project slug {slug:?}: {e}")))?;
        task = task.with_project_id(pid);
    }
    let task_id = task.id;
    state.fuxi.dispatch(xuannv, task).await?;

    Ok(Json(DispatchResponse {
        task_id: task_id.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use fuxi_core::id::AgentId;
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

    async fn build_app() -> (tempfile::TempDir, Router, Arc<Fuxi>) {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let (dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi.clone());
        let app = Router::new()
            .route("/api/dispatch", post(dispatch))
            .with_state(state);
        (dir, app, fuxi)
    }

    fn req(title: &str, description: &str) -> Request<Body> {
        let body = serde_json::json!({ "title": title, "description": description });
        Request::builder()
            .method("POST")
            .uri("/api/dispatch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn returns_503_when_xuannv_not_set() {
        let (_dir, app, _) = build_app().await;
        let resp = app
            .oneshot(req("修 ERP-1066", "复现 + 修 + 提 PR"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn returns_503_when_xuannv_id_set_but_agent_not_in_shelf() {
        let (_dir, app, fuxi) = build_app().await;
        fuxi.set_xuannv(AgentId::new()).await;
        let resp = app.oneshot(req("title", "desc")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// v2 跨节点：非法 project slug 应 400。合法 slug + 玄女未起仍 503——验
    /// project 解析在 xuannv 检查之前不变，错误码语义不冲突。
    #[tokio::test]
    async fn rejects_invalid_project_slug() {
        let (_dir, app, fuxi) = build_app().await;
        fuxi.set_xuannv(AgentId::new()).await;
        let body = serde_json::json!({
            "title": "t",
            "description": "d",
            "project": "BAD-SLUG"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/dispatch")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_empty_title_or_description() {
        let (_dir, app, _) = build_app().await;
        let resp = app.clone().oneshot(req("", "desc")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp = app.clone().oneshot(req("title", "  ")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // 注：派活"真成功路径"（task_id 返回 + TaskCreated/Dispatched 入 EventBus）
    // 需要 shelf 里有真 agent，而 `Fuxi::spawn_worker` 会真起 cc 进程，unit test
    // 太重。这条交给 ζ Task #11 的 e2e 部署验证（启 daemon → /api/login → 真 spawn
    // → /api/dispatch → 看 task_id 回返 + WebSocket /api/tasks/:id/stream 看事件）。
    //
    // 当前 unit 测试覆盖了三条主路径（玄女未起 / 注册了但 shelf 无 / 空字段），
    // 真 happy path 借 e2e 的 reqwest+axum 跑（已在 tests/ws_stream.rs 同款模板
    // 里有），但需要起 cc——v1 ship 不阻塞，#11 e2e 兜底。
}
