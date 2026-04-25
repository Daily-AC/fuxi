//! `POST /api/intervene` —— PWA 顶部"跟玄女说"输入条的出口。
//!
//! 流程：
//! 1. 取 `state.fuxi.xuannv_id().await` —— 没起 → 503（PWA 应在 LoginView 后
//!    第一次访问就被 ζ 的 `ensure_xuannv` 自启拉起；理论上不该 503，但兜底）
//! 2. 调 `Fuxi::intervene(xuannv, interrupt, text)`
//!    - Idle → 自动 degrade 成 dispatch（Decision 04，Fuxi 内部已实装）
//!    - Busy → enqueue 到 pending（M2.1 已实装）
//! 3. 返 200 `{ "ok": true }`
//!
//! 决策 04 的 degrade 在 Fuxi 层完成——**IM handler 不要自己再判 idle/busy**。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct InterveneBody {
    /// 用户输入的文字。允许空白？拒空——空白 turn 没意义。
    pub text: String,
    /// 是否打断当前 turn（决策 04 中 `interrupt_first` 语义）。默认 false 走追加。
    #[serde(default)]
    pub interrupt: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InterveneResponse {
    pub ok: bool,
}

pub async fn intervene(
    State(state): State<AppState>,
    Json(body): Json<InterveneBody>,
) -> Result<Json<InterveneResponse>> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(Error::BadRequest("text 不能为空".into()));
    }

    let xuannv =
        state.fuxi.xuannv_id().await.ok_or_else(|| {
            Error::Unavailable("玄女尚未就绪——请稍后重试或检查 daemon 启动".into())
        })?;

    state.fuxi.intervene(xuannv, body.interrupt, text).await?;

    Ok(Json(InterveneResponse { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
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
            .route("/api/intervene", post(intervene))
            .with_state(state);
        (dir, app, fuxi)
    }

    fn req(text: &str, interrupt: bool) -> Request<Body> {
        let body = serde_json::json!({ "text": text, "interrupt": interrupt });
        Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    /// 玄女未起 → 503 + error=unavailable（PWA 应据此重试 / 显示等待提示）。
    #[tokio::test]
    async fn returns_503_when_xuannv_not_set() {
        let (_dir, app, _) = build_app().await;
        let resp = app.oneshot(req("hello", false)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "unavailable");
        let msg = parsed["message"].as_str().unwrap_or("");
        assert!(
            msg.contains("玄女") && msg.contains("就绪"),
            "提示文案应明示，得到：{msg}"
        );
    }

    /// 玄女 set 了但 shelf 里没真 agent → AgentNotFound → 503。
    /// 这条覆盖"玄女 id 注册过但 spawn 后被 shutdown / agent record 丢"边界。
    #[tokio::test]
    async fn returns_503_when_xuannv_id_set_but_agent_not_in_shelf() {
        let (_dir, app, fuxi) = build_app().await;
        // 注一个假 id（shelf 里没这只 agent）
        fuxi.set_xuannv(AgentId::new()).await;
        let resp = app.oneshot(req("hello", false)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "xuannv_unavailable");
    }

    #[tokio::test]
    async fn rejects_empty_text() {
        let (_dir, app, _) = build_app().await;
        let resp = app.oneshot(req("   ", false)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
