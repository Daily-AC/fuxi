//! `POST /api/intervene` —— PWA 顶部"跟玄女说"输入条的出口。
//!
//! 流程：
//! 1. 取 `state.fuxi.xuannv_id().await` —— 没起 → 503（PWA 应在 LoginView 后
//!    第一次访问就被 ζ 的 `ensure_xuannv` 自启拉起；理论上不该 503，但兜底）
//! 2. 解出路由 target：
//!    - body.target 显式指定 → 用它（PWA 任务 thread 里 @ 门客的场景）
//!    - 否则 → 玄女（PWA 玄女 tab 默认对话）
//! 3. 调 `Fuxi::intervene(target, interrupt, text, mentions)`
//!    - Idle → 自动 degrade 成 dispatch（Decision 04，Fuxi 内部已实装）
//!    - Busy → enqueue 到 pending（M2.1 已实装）
//! 4. 返 200 `{ "ok": true }`
//!
//! 决策 04 的 degrade 在 Fuxi 层完成——**IM handler 不要自己再判 idle/busy**。
//!
//! v3 #N7'（spec `2026-04-26-im-tab-bar-task-thread-design.md` §intervene 字段扩展）：
//! body 增加可选 `target` 和 `mentions`：
//! - `target`: 路由目标 agent_id；前端约定 = `mentions[0]`（无 @ 时省略 = 玄女）
//! - `mentions`: 所有 @ 的 agent_id（含 target 自身），写入事件供历史还原 chip
//! - v1 不实装 fan-out 通知——`mentions[1..]` 仅作 mention 标记

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use fuxi_core::AgentId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct InterveneBody {
    /// 用户输入的文字。允许空白？拒空——空白 turn 没意义。
    pub text: String,
    /// 是否打断当前 turn（决策 04 中 `interrupt_first` 语义）。默认 false 走追加。
    #[serde(default)]
    pub interrupt: bool,
    /// 路由目标 agent_id；缺省 → 玄女。v3 #N7'：前端任务 thread 里 @ 门客时填。
    #[serde(default)]
    pub target: Option<AgentId>,
    /// 所有被 @ 的 agent_id（含 target 自身）。仅写入事件供前端历史还原 chip 视觉，
    /// **后端 v1 不据此 fan-out 通知**——只 `target` 有路由效果。
    #[serde(default)]
    pub mentions: Vec<AgentId>,
    /// 用户在 PWA composer 用 `@<node_id>` 显式 pin 到的 dist 节点（如
    /// `mac-local`）。β · #57：写入 `EventKind::UserInterventionSent.pinned_node`
    /// 供历史回放还原节点 chip 视觉；真路由要等 task 维度的 dispatch routing 决策树
    /// 落地（target=local agent + 退化 dispatch 时把 pinned_node 注入 task）。
    /// v1 暂仅记录，留 v1.x dispatch routing 一并消费。
    #[serde(default)]
    pub pinned_node: Option<String>,
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

    // target 解析：body 显式 → 用之；否则 fallback 玄女
    let target = match body.target {
        Some(t) => t,
        None => state.fuxi.xuannv_id().await.ok_or_else(|| {
            Error::Unavailable("玄女尚未就绪——请稍后重试或检查 daemon 启动".into())
        })?,
    };

    state
        .fuxi
        .intervene(
            target,
            body.interrupt,
            text,
            body.mentions,
            body.pinned_node,
        )
        .await?;

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

    /// v3 #N7'：body.target 显式指定 → 走该 target，**不**回退玄女。
    /// 用 shelf 里没有的 target id：handler 应跳过 xuannv lookup 直接调
    /// `Fuxi::intervene(target, ...)`，结果 AgentNotFound → 503。
    /// （路由路径正确就够；不真起 agent 验响应）
    #[tokio::test]
    async fn body_target_routes_to_explicit_agent_not_xuannv() {
        let (_dir, app, fuxi) = build_app().await;
        // 注一个假玄女——若 handler 错误地走了 xuannv 路径会撞 xuannv_unavailable
        fuxi.set_xuannv(AgentId::new()).await;

        let target = AgentId::new();
        let body = serde_json::json!({
            "text": "hi worker",
            "interrupt": false,
            "target": target,
            "mentions": [target],
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // shelf 里没这个 agent → 503，但 error 应是 agent_not_found 系而非 xuannv_unavailable
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let err = parsed["error"].as_str().unwrap_or("");
        assert_ne!(
            err, "unavailable",
            "走了 xuannv 错误路径，应该按 body.target 路由"
        );
    }

    #[tokio::test]
    async fn rejects_empty_text() {
        let (_dir, app, _) = build_app().await;
        let resp = app.oneshot(req("   ", false)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// 老 PWA / TUI 客户端发的 body 不带 `target` `mentions`——两字段 `#[serde(default)]`，
    /// 应当反序列化成 `target=None` `mentions=vec![]`，行为同现状。
    #[test]
    fn body_deserializes_without_target_or_mentions_for_back_compat() {
        let raw = serde_json::json!({ "text": "hi", "interrupt": false });
        let body: InterveneBody = serde_json::from_value(raw).expect("legacy body");
        assert_eq!(body.text, "hi");
        assert!(!body.interrupt);
        assert!(body.target.is_none());
        assert!(body.mentions.is_empty());
    }

    /// v3 #N7' 完整 body 形态——target + mentions 都解析。
    #[test]
    fn body_deserializes_with_target_and_mentions() {
        let target = AgentId::new();
        let other = AgentId::new();
        let raw = serde_json::json!({
            "text": "查 ERP-1066",
            "target": target,
            "mentions": [target, other],
        });
        let body: InterveneBody = serde_json::from_value(raw).expect("body");
        assert_eq!(body.target, Some(target));
        assert_eq!(body.mentions, vec![target, other]);
    }
}
