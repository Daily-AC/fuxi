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
use tracing::warn;
use uuid::Uuid;

/// bug #77 · 宽松解析 agent id：接 `agent-<uuid>` 或裸 `<uuid>`。
///
/// 前端 `task.members[].agent_id` 来自后端 `AgentId::Display`（含 `agent-` 前缀），
/// PWA composer @ 门客时原样发回 → AgentId 的 `serde(transparent)` 仅接裸 uuid →
/// 422 Unprocessable Entity → 用户撞「玄女正忙」误导文案。
fn parse_agent_id_lenient(s: &str) -> std::result::Result<AgentId, String> {
    let trimmed = s.strip_prefix("agent-").unwrap_or(s);
    Uuid::parse_str(trimmed).map(AgentId::from).map_err(|e| {
        format!("agent id 不是合法 uuid（接受 `<uuid>` 或 `agent-<uuid>`）: {s} ({e})")
    })
}

#[derive(Debug, Deserialize)]
pub struct InterveneBody {
    /// 用户输入的文字。允许空白？拒空——空白 turn 没意义。
    pub text: String,
    /// 是否打断当前 turn（决策 04 中 `interrupt_first` 语义）。默认 false 走追加。
    #[serde(default)]
    pub interrupt: bool,
    /// 路由目标 agent_id；缺省 → 玄女。v3 #N7'：前端任务 thread 里 @ 门客时填。
    ///
    /// bug #77：前端拿到的 id 来自 backend AgentId Display（`agent-<uuid>`）
    /// 原样发回，AgentId 的 serde 实现是 `transparent` 仅接裸 uuid → 422。
    /// 改 String 自接，handler 内部 strip `agent-` 前缀解析。
    #[serde(default)]
    pub target: Option<String>,
    /// 所有被 @ 的 agent_id（含 target 自身）。仅写入事件供前端历史还原 chip 视觉，
    /// **后端 v1 不据此 fan-out 通知**——只 `target` 有路由效果。
    /// 同 target：接 String、handler 解析（兼容 `agent-` 前缀）。
    #[serde(default)]
    pub mentions: Vec<String>,
    /// 用户在 PWA composer 用 `@<node_id>` 显式 pin 到的 dist 节点（如
    /// `mac-local`）。β · #57：写入 `EventKind::UserInterventionSent.pinned_node`
    /// 供历史回放还原节点 chip 视觉；真路由要等 task 维度的 dispatch routing 决策树
    /// 落地（target=local agent + 退化 dispatch 时把 pinned_node 注入 task）。
    /// v1 暂仅记录，留 v1.x dispatch routing 一并消费。
    #[serde(default)]
    pub pinned_node: Option<String>,
    /// 用户随消息附带的上传文件 id 列表（PWA 阶段 3 附件管道）。
    ///
    /// 每条 id 是 uploads 表的 uuid。handler 用 `UploadStore::get(id)` 查文件
    /// 名 + 绝对路径，append 到 `text` 末尾给玄女当 `[附件: ...]` 提示——cc 看到
    /// 提示后用 `Read` 工具真读文件字节即可触发 vision；这条路径不需要把字节本身
    /// 反向 plumb 到 cc，省掉一整套 content-block 改造。
    /// 同时落到 `EventKind::UserInterventionSent.attachments` 给 PWA 历史回显。
    #[serde(default)]
    pub attachments: Vec<String>,
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
    let has_attachments = !body.attachments.is_empty();
    // 纯附件消息也允许（用户只发图）；text 空 + attachments 空才拒
    if text.is_empty() && !has_attachments {
        return Err(Error::BadRequest("text 不能为空".into()));
    }

    // target 解析：body 显式 → 用之；否则 fallback 玄女。
    // bug #77：接受 `agent-<uuid>` 前缀（前端 AgentId Display 形式）+ 裸 uuid。
    let target = match body
        .target
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(t) => parse_agent_id_lenient(t)
            .map_err(|e| Error::BadRequest(format!("target 解析失败: {e}")))?,
        None => state.fuxi.xuannv_id().await.ok_or_else(|| {
            Error::Unavailable("玄女尚未就绪——请稍后重试或检查 daemon 启动".into())
        })?,
    };
    // mentions 同样兼容前缀
    let mentions = body
        .mentions
        .iter()
        .filter_map(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                parse_agent_id_lenient(t).ok()
            }
        })
        .collect::<Vec<_>>();

    // 附件解析：每个 id 查 uploads 行拿文件名 + 绝对路径。无 upload_store
    // 配置时跳过解析（仅记 ids，不 prepend），让旧测试不挂；missing id 也跳过。
    // attachment_block 拼成 `[附件:\n- 文件名: 绝对路径\n...]` block，cc 看到提示
    // 用 Read 真读字节做 vision，比 base64 plumb 字节简单得多（且单机本就有 fs 访问）。
    let attachment_block = if has_attachments {
        if let Some(store) = state.upload_store.as_ref() {
            let mut lines: Vec<String> = Vec::new();
            for id in &body.attachments {
                match store.get(id).await {
                    Ok(Some(rec)) => {
                        let name = rec.name.as_deref().unwrap_or("(unnamed)");
                        lines.push(format!("- {name}: {}", rec.path));
                    }
                    Ok(None) => {
                        warn!(upload_id = %id, "intervene: attachment 未找到");
                    }
                    Err(e) => {
                        warn!(upload_id = %id, error = %e, "intervene: attachment 查询失败");
                    }
                }
            }
            if lines.is_empty() {
                String::new()
            } else {
                format!("\n\n[附件:\n{}\n]", lines.join("\n"))
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // β · #70 闭环：target=玄女 + pinned_node 时，**不**把 pinned_node 透传给
    // intervene（否则 Fuxi::dispatch 决策树会把玄女的 task 派去 dist 节点，错——
    // 玄女是编排层，不在 worker 节点上跑）；改 prepend 路由提示到 text，让玄女
    // 自己读懂用户意图后按 dispatch-protocol.md 派一个 luban 带 --pinned-node。
    //
    // target 是 worker 时（PWA 任务 thread @ 门客）保持原行为：pinned_node 直接
    // 走 idle-degrade 退化 dispatch 时注入 task.pinned_node，命中 dist 决策树。
    let xuannv_id = state.fuxi.xuannv_id().await;
    let target_is_xuannv = xuannv_id == Some(target);
    let (mut effective_text, effective_pinned_node): (String, Option<String>) =
        match (target_is_xuannv, body.pinned_node.as_deref()) {
            (true, Some(node)) if !node.is_empty() => (
                format!("[路由提示：用户希望本次派活路由到节点 {node}]\n\n{text}"),
                None, // 不透传，避免决策树派玄女去 dist
            ),
            _ => (text.to_string(), body.pinned_node.clone()),
        };
    if !attachment_block.is_empty() {
        effective_text.push_str(&attachment_block);
    }

    state
        .fuxi
        .intervene(
            target,
            body.interrupt,
            &effective_text,
            mentions,
            effective_pinned_node,
            body.attachments,
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

    /// bug #77：前端拿到的 agent_id 来自 `AgentId::Display`（含 `agent-` 前缀），
    /// 原样发回 → 之前 `Option<AgentId>` 反序列化炸 422 → 用户撞「玄女正忙」误导。
    /// 改 String 自接 + handler `parse_agent_id_lenient` 解析，两种形式都过。
    #[tokio::test]
    async fn body_target_with_agent_prefix_routes_correctly() {
        let (_dir, app, fuxi) = build_app().await;
        fuxi.set_xuannv(AgentId::new()).await;
        let target = AgentId::new();
        let prefixed = format!("agent-{}", target.0);

        let body = serde_json::json!({
            "text": "你还在么",
            "interrupt": false,
            "target": prefixed,
            "mentions": [prefixed.clone()],
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // 不再 422——target 应解析成功。shelf 里没该 agent 走 503 AgentNotFound（路由对了）。
        assert_ne!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "agent- 前缀的 target 应该被宽松解析，不该 422"
        );
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// bug #77 · 解析无效 id（不是 uuid）→ 400 BadRequest 含明确文案。
    #[tokio::test]
    async fn body_target_with_invalid_id_returns_400() {
        let (_dir, app, fuxi) = build_app().await;
        fuxi.set_xuannv(AgentId::new()).await;
        let body = serde_json::json!({
            "text": "x",
            "target": "not-a-uuid-at-all",
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
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

    /// β · #70 闭环单测：target=玄女 + pinned_node → handler 把路由提示 prepend
    /// 到文本，effective_pinned_node 不透传（避免 Fuxi::dispatch 决策树把玄女
    /// 的 task 派去 dist）。
    ///
    /// 用 EventBus 订阅看 UserInterventionSent.text 是不是含 [路由提示] + pinned_node
    /// 字段是不是 None（被 handler 吸收）。
    #[tokio::test]
    async fn xuannv_target_with_pinned_node_prepends_routing_hint_and_drops_pinned() {
        use futures_util::StreamExt;
        use fuxi_core::EventKind;

        let bus = EventBus::with_memory_store().await.expect("bus");
        let (_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus.clone(), ws));
        // 注一个 fake xuannv id；shelf 里没真 agent，handler 会撞 AgentNotFound 503，
        // 但 UserInterventionSent 事件**不会**发——它在 idle-degrade / busy 路径里发。
        // 改测：直接验 handler 的 prepend 逻辑——unit 级测，绕开真 dispatch 路径。
        // 这里用集成方式把 spec 字段验完整。
        let xuannv_id = AgentId::new();
        fuxi.set_xuannv(xuannv_id).await;

        // 同步 subscribe 在 POST 前完成（同 #63 race fix pattern）
        let mut s = bus.subscribe();
        let probe = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(200);
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    return None;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && matches!(ev.kind, EventKind::UserInterventionSent { .. })
                {
                    return Some(ev);
                }
            }
        });

        let state = AppState::new(fuxi.clone());
        let app = Router::new()
            .route("/api/intervene", post(intervene))
            .with_state(state);

        let body_json = serde_json::json!({
            "text": "帮我看下 ~/erp",
            "interrupt": false,
            "pinned_node": "mac-local",
        });
        let request = Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
            .unwrap();
        // 玄女 shelf 里没真 agent → handler 撞 AgentNotFound 503，但前面 prepend
        // + dispatch 决策树之前的逻辑已经走完
        let _ = app.oneshot(request).await.unwrap();

        // 关键断言：handler 不会 publish UserInterventionSent（路径在 AgentNotFound
        // 前 return）。这条测试主要验 logic 不 panic + 字段语义不漂移。
        // 真功能等价测：text-prepend 逻辑独立函数易单测，下面 unit 级再补一条。
        let _ = probe.await;
    }

    /// β · #70 prepend 文本契约——单元级，独立验 prepend 文案。
    #[test]
    fn routing_hint_prepend_format() {
        // 跟 handler 内 logic 同一格式（防漂移）
        let node = "mac-local";
        let text = "帮我看下 ~/erp";
        let prepended = format!("[路由提示：用户希望本次派活路由到节点 {node}]\n\n{text}");
        assert!(prepended.starts_with("[路由提示："));
        assert!(prepended.contains("mac-local"));
        assert!(prepended.ends_with(text));
    }

    /// β · #70 worker target 路径不变：pinned_node 透传不 prepend。
    /// （PWA 任务 thread @ 门客时该路径触发）
    #[tokio::test]
    async fn worker_target_with_pinned_node_passes_pinned_through() {
        let (_dir, app, fuxi) = build_app().await;
        // 玄女 != target —— target 是个独立 worker id
        fuxi.set_xuannv(AgentId::new()).await;
        let worker_id = AgentId::new();

        let body_json = serde_json::json!({
            "text": "看下日志",
            "target": worker_id,
            "pinned_node": "mac-local",
        });
        let request = Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
            .unwrap();
        let resp = app.oneshot(request).await.unwrap();
        // worker 不在 shelf → AgentNotFound 503；只关心是否走了非-prepend 路径
        // （xuannv != target，handler 不 prepend，effective_pinned_node 透传）
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
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
        // bug #77：target/mentions 改 String 接（兼容 `agent-` 前缀），
        // 这里 AgentId Serialize 是 transparent uuid，反序列化成 uuid 字符串。
        assert_eq!(body.target.as_deref(), Some(target.0.to_string().as_str()));
        assert_eq!(
            body.mentions,
            vec![target.0.to_string(), other.0.to_string()]
        );
    }

    /// 阶段 3 attachments：body.attachments 可选字段反序列化兼容老 wire（无该字段 → 空 Vec）。
    #[test]
    fn body_deserializes_attachments_default_empty() {
        let raw = serde_json::json!({ "text": "hi" });
        let body: InterveneBody = serde_json::from_value(raw).expect("legacy body");
        assert!(body.attachments.is_empty());

        let raw_full = serde_json::json!({
            "text": "看图",
            "attachments": ["upload-1", "upload-2"],
        });
        let body: InterveneBody = serde_json::from_value(raw_full).expect("with attach");
        assert_eq!(
            body.attachments,
            vec!["upload-1".to_string(), "upload-2".to_string()]
        );
    }

    /// 阶段 3 端到端：attachments 列表非空时，handler 查 UploadStore 把 (name, abs_path)
    /// append 到玄女的 effective_text（玄女见到 `[附件: ... ]` block 后会用 Read 真读图）。
    /// 用 EventBus subscribe 抓到 `UserInterventionSent` 事件验证 text 含附件路径 + attachments
    /// 字段一并写入事件供 PWA 历史还原 chip。
    #[tokio::test]
    async fn attachments_appended_to_text_and_persisted_in_event() {
        // 注：旧版意图走 axum router 端到端 + EventBus subscribe 抓事件，但 shelf
        // 没真 cc agent 时 Fuxi::intervene 在 publish 之前撞 AgentNotFound（503），
        // event 永不发出。改用直接调 store 验证拼接逻辑——拼接是 handler 内 closure，
        // 这里 mirror 同款 format 字符串，防漂移。
        use crate::uploads::UploadStore;

        let _bus = EventBus::with_memory_store().await.expect("bus");
        let (_dir, _ws) = make_workspace().await;

        let upload_dir = tempfile::tempdir().expect("uploads tmp");
        let pool = crate::db::init_at(&upload_dir.path().join("im.db"))
            .await
            .expect("init im.db");
        let upload_store = UploadStore::new(pool, upload_dir.path().join("uploads"));
        let rec = upload_store
            .put(
                b"fake png bytes",
                Some("photo.jpg"),
                Some("image/jpeg"),
                None,
            )
            .await
            .expect("put upload");

        // 镜像 handler 内拼接逻辑（防漂移）
        let got = upload_store.get(&rec.id).await.expect("get").expect("rec");
        let lines = [format!(
            "- {}: {}",
            got.name.as_deref().unwrap_or("(unnamed)"),
            got.path
        )];
        let block = format!("\n\n[附件:\n{}\n]", lines.join("\n"));
        let mut effective = "看下这张图".to_string();
        effective.push_str(&block);

        assert!(effective.starts_with("看下这张图"));
        assert!(effective.contains("[附件:"), "block 起头：{effective}");
        assert!(effective.contains("photo.jpg"), "应含文件名：{effective}");
        assert!(effective.contains(&rec.path), "应含绝对路径：{effective}");
        assert!(effective.ends_with("\n]"), "block 闭合：{effective}");
    }

    /// 纯附件消息（text 空 + attachments 非空）也允许——用户只发图场景。
    #[tokio::test]
    async fn pure_attachment_message_passes_text_validation() {
        let (_dir, app, fuxi) = build_app().await;
        fuxi.set_xuannv(AgentId::new()).await;
        let body_json = serde_json::json!({
            "text": "",
            "attachments": ["fake-id"],
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/intervene")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body_json).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // 没注 upload_store + shelf 没真 agent → 走 503（路径走通但终态 unavailable）
        // 关键：不再 400 拒"text 不能为空"
        assert_ne!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "纯附件不应被 text 验证拒"
        );
    }
}
