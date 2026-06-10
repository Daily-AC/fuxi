//! `/api/conv` + `/api/tasks/:id/stream` —— WebSocket 事件流。
//!
//! `/api/conv`（玄女对话流，Phase 2 池版）：
//!   - 仅推「池中活分身且其归属 topic == 当前 topic」的事件——分身相关的 EventKind
//!     （`UserPrompted` / `AgentResponded` / `OrchestratorCcReceived` /
//!     `UserInterventionSent` / `AgentInterrupted` / `ThinkingStarted/Finished` /
//!     `ToolCallStarted/Finished`）都会带 `meta.agent = 某分身`。按「是不是池内分身 +
//!     归属 topic 匹配」过滤（[`conv_event_visible`]），**不维护一份 EventKind 白名单**
//!     ——哪天加新 EventKind，只要 publisher 正确 set `meta.agent` 就自动出现，不改 IM。
//!   - 非当前 topic 分身的主动汇报被本过滤拦下，走 conv_store 落库 + 未读小红点 +
//!     push 三件套，不插进当前视图实时流。
//!
//! `/api/tasks/:id/stream`（单任务流）：
//!   - 仅推 `meta.task == Some(:id)` 的事件——任务级实时观察。
//!
//! 两者都接受 `?from=<event_id|rfc3339>` 做历史 + live tail；客户端断线自带 cursor
//! 重连——服务端不维护 session（这是 firehose 同款契约）。

use crate::conv_store::{Message, SCOPE_XUANNV};
use crate::error::{Error, Result};
use crate::handlers::ws_common::{build_event_stream, parse_cursor, run_ws_loop};
use crate::state::AppState;
use axum::Json;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use fuxi_core::TaskId;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

/// 解析路径里的 task id——接受裸 UUID 或 `task-<uuid>` Display 形式。
/// 前者给 PWA 短 URL 用；后者兼容 `AgentId::to_string()` 同款契约。
fn parse_task_id(s: &str) -> std::result::Result<TaskId, String> {
    let trimmed = s.strip_prefix("task-").unwrap_or(s);
    Uuid::parse_str(trimmed)
        .map(TaskId::from)
        .map_err(|e| format!("task id 不是合法的 UUID: {s} ({e})"))
}

/// `?from=<cursor>` 公共 query。
#[derive(Debug, Default, Deserialize)]
pub struct StreamQuery {
    /// 回放游标——事件 UUID 或 RFC3339 时间戳。缺省 = live-only。
    pub from: Option<String>,
}

/// 玄女对话流过滤（Phase 2）：只放行「池中活分身」的事件，且分身归属 topic ==
/// 当前 topic。归属用池槽位反查——分身自己的事件不一定带 meta.topic_id，池是
/// 「谁服务哪个 topic」的唯一真相源。B 分身的主动汇报被本过滤拦下，走
/// 落库 + 未读小红点 + push 三件套，不插进 A 的实时视图。
pub(crate) fn conv_event_visible(
    pool: &std::collections::HashMap<fuxi_core::TopicId, fuxi_core::id::AgentId>,
    current: fuxi_core::TopicId,
    ev: &fuxi_core::event::Event,
) -> bool {
    let Some(agent) = ev.meta.agent else {
        return false;
    };
    let Some(topic) = pool.iter().find_map(|(t, a)| (*a == agent).then_some(*t)) else {
        return false;
    };
    topic == current
}

/// `WS /api/conv` —— 玄女对话事件流。
#[tracing::instrument(skip(ws, state))]
pub async fn conv_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Result<Response> {
    let cursor = parse_cursor(q.from.as_deref())?;
    // Phase 2：filter 改池 watch + current_topic watch 双实时——分身 respawn 漂移、
    // 用户切 topic 都即时跟随（绝不 snapshot，见 memory
    // `feedback_dynamic_agent_id_via_watch`：长连 WS 不会因漂移重 accept，烤死的
    // 旧值会让对应分身的 AgentResponded 全被静默滤掉，live 流断供）。503 判断保留
    // general 镜像：general 永驻（idle_gc 豁免），镜像 None = 玄女体系尚未 boot。
    let pool_watch = state.fuxi.xuannv_pool_watch();
    let topic_watch = state.fuxi.current_topic_watch();
    let id_watch = state.fuxi.xuannv_id_watch();
    let xuannv_now = *id_watch.borrow();
    if xuannv_now.is_none() {
        // 玄女还没起——前端应能区分"暂时无对话流"和"路由错"。返 503 比 400 合适，
        // 但本 crate 错误枚举只有 NOT_FOUND/UNAUTHORIZED/...，先归 NotFound 语义最近。
        return Err(Error::NotFound("玄女尚未注入；请先 set_xuannv".into()));
    }
    info!(?cursor, xuannv = ?xuannv_now, "ws /api/conv accept");

    let bus = state.fuxi.bus().clone();
    let stream = build_event_stream(&bus, cursor);

    let resp = ws.on_upgrade(move |socket| async move {
        run_ws_loop(socket, stream, move |ev| {
            conv_event_visible(&pool_watch.borrow(), *topic_watch.borrow(), ev)
        })
        .await;
    });
    Ok(resp)
}

/// `GET /api/conv/messages` —— IM 层聊天记录拉取（Task #17）。
///
/// 与 `WS /api/conv` 的实时事件流互补：WS 推未来发生的，本端点拉**历史**消息。
/// 前端 LoginView 后首屏加载、用户向上滚动加载更早消息都走这里。
#[derive(Debug, Default, Deserialize)]
pub struct MessagesQuery {
    /// 指定 conversation scope。默认 `"xuannv"` 主线；任务子线传 `"task:<task_id>"`。
    #[serde(default)]
    pub conv: Option<String>,
    /// 一页最多多少条；默认 50，硬上限 200（store 内部 clamp）。
    #[serde(default)]
    pub limit: Option<usize>,
    /// 翻历史 cursor —— 上一页 oldest message id。`None` = 最新一页。
    #[serde(default)]
    pub before: Option<String>,
    /// Phase 1 · 玄女主对话切 topic 时按 UUID 过滤；缺省走 server-side
    /// `state.fuxi.current_topic_id()`（PWA 不必主动传，老 client 默认行为对齐）。
    /// 任务子线（`conv=task:<id>`）传不传都无所谓——任务 thread 跨 topic 不分。
    #[serde(default)]
    pub topic_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub messages: Vec<Message>,
    pub has_more: bool,
    /// 本页最早消息 id；`None` 表示空页。前端记下当作下次 `before`。
    pub oldest: Option<String>,
}

pub async fn messages(
    State(state): State<AppState>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<MessagesResponse>> {
    let store = state.conv_store.as_ref().ok_or_else(|| {
        Error::Unavailable("conv_store 未注入：daemon 启动期 with_conv_store".into())
    })?;
    let scope = q.conv.as_deref().unwrap_or(SCOPE_XUANNV);
    let conv_id = store.ensure_scope(scope, None).await?;
    let limit = q.limit.unwrap_or(50);
    // Phase 1 · topic 过滤仅对玄女主线生效——任务子线（conv=task:<id>）跨 topic 不分。
    // 玄女主线缺省走当前 topic，避免老 PWA 不传 topic_id 仍拉全 conv 漏旧 topic 历史。
    let (mut messages, has_more, oldest) = if scope == SCOPE_XUANNV {
        let topic = match q.topic_id.as_deref() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => state.fuxi.current_topic_id().0.to_string(),
        };
        store
            .page_messages_in_topic(&conv_id, &topic, limit, q.before.as_deref())
            .await?
    } else {
        store
            .page_messages(&conv_id, limit, q.before.as_deref())
            .await?
    };
    // v1-session19 #2 · hydrate 附件元数据 — 让前端历史回放显真名而非 uuid。
    // upload_store 缺时（少数 smoke 路径）跳过：前端 fallback uploadsFromIds(id 当 name)
    // 不致命，只是显示退化。
    if let Some(upload_store) = state.upload_store.as_ref() {
        hydrate_attachments(&mut messages, upload_store).await;
    }
    Ok(Json(MessagesResponse {
        messages,
        has_more,
        oldest,
    }))
}

/// 把 messages 里 attachments JSON 数组中的 id 批量 lookup upload_store，
/// 填到 message.attachment_uploads。lookup 失败的 id 静默跳过（前端 fallback）。
async fn hydrate_attachments(messages: &mut [Message], upload_store: &crate::uploads::UploadStore) {
    use std::collections::{BTreeSet, HashMap};
    let mut all_ids: BTreeSet<String> = BTreeSet::new();
    for m in messages.iter() {
        let Some(att) = &m.attachments else {
            continue;
        };
        let Some(arr) = att.as_array() else { continue };
        for v in arr {
            if let Some(s) = v.as_str() {
                all_ids.insert(s.to_string());
            }
        }
    }
    if all_ids.is_empty() {
        return;
    }
    let mut by_id: HashMap<String, crate::uploads::UploadDigest> =
        HashMap::with_capacity(all_ids.len());
    for id in &all_ids {
        match upload_store.get(id).await {
            Ok(Some(rec)) => {
                by_id.insert(id.clone(), rec.into());
            }
            Ok(None) => {
                // 已删除 / 异常 id — 前端 fallback 显 id 当 name
            }
            Err(e) => {
                tracing::warn!(error = %e, id = %id, "hydrate_attachments: upload lookup 失败");
            }
        }
    }
    for m in messages.iter_mut() {
        let Some(att) = &m.attachments else { continue };
        let Some(arr) = att.as_array() else { continue };
        for v in arr {
            if let Some(s) = v.as_str()
                && let Some(d) = by_id.get(s)
            {
                m.attachment_uploads.push(d.clone());
            }
        }
    }
}

/// `WS /api/tasks/{id}/stream` —— 单任务事件流。
#[tracing::instrument(skip(ws, state), fields(task_id = %id))]
pub async fn task_stream_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Result<Response> {
    let task_id = parse_task_id(&id).map_err(|e| {
        warn!(raw = %id, error = %e, "task_id 解析失败");
        Error::BadRequest(e)
    })?;
    let cursor = parse_cursor(q.from.as_deref())?;
    info!(?cursor, "ws /api/tasks/{id}/stream accept");

    let bus = state.fuxi.bus().clone();
    let stream = build_event_stream(&bus, cursor);

    let resp = ws.on_upgrade(move |socket| async move {
        run_ws_loop(socket, stream, move |ev| ev.meta.task == Some(task_id)).await;
    });
    Ok(resp)
}

#[cfg(test)]
mod tests {
    //! 只覆盖 `messages` HTTP handler 的契约——WS handler 走 tests/ws_stream.rs 的
    //! e2e 真 socket 测试。

    use super::*;
    use crate::conv_store::ConvStore;
    use crate::db::init_at;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get as axum_get;
    use chrono::Utc;
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

    async fn build_app() -> (tempfile::TempDir, Router, ConvStore) {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(&dir.path().join("im.db")).await.expect("init");
        let store = ConvStore::new(pool);

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_conv_store(store.clone());
        let app = Router::new()
            .route("/api/conv/messages", axum_get(super::messages))
            .with_state(state);
        (dir, app, store)
    }

    fn req(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn empty_returns_empty_array_no_more() {
        let (_dir, app, _store) = build_app().await;
        let resp = app
            .oneshot(req("/api/conv/messages?conv=xuannv&limit=20"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed["messages"].as_array().unwrap().is_empty());
        assert_eq!(parsed["has_more"], false);
        assert!(parsed["oldest"].is_null());
    }

    #[tokio::test]
    async fn paginates_with_before_cursor() {
        let (_dir, app, store) = build_app().await;
        let conv_id = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let base = Utc::now();
        for i in 0..5 {
            store
                .append_message(
                    &conv_id,
                    "user",
                    None,
                    "text",
                    &serde_json::json!({"text": format!("m-{i}")}),
                    None,
                    None,
                    base + chrono::Duration::milliseconds(i),
                )
                .await
                .unwrap();
        }

        // 拿最新 3 条
        let resp = app
            .clone()
            .oneshot(req("/api/conv/messages?conv=xuannv&limit=3"))
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msgs = parsed["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(parsed["has_more"], true);
        let oldest = parsed["oldest"].as_str().unwrap().to_string();

        // 翻历史
        let url = format!("/api/conv/messages?conv=xuannv&limit=3&before={oldest}");
        let resp = app.oneshot(req(&url)).await.unwrap();
        let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msgs = parsed["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2, "更早只剩 2 条");
        assert_eq!(parsed["has_more"], false);
    }

    /// v1-session19 #2 · hydrate 附件元数据测试 —— 老消息 attachments 是 ID 数组，
    /// handler 应批量 lookup upload_store 填 attachment_uploads，让前端拿真名。
    #[tokio::test]
    async fn hydrates_attachment_uploads_from_upload_store() {
        use crate::uploads::UploadStore;

        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(&dir.path().join("im.db")).await.expect("init");
        let conv = ConvStore::new(pool.clone());
        let upload_store = UploadStore::new(pool, dir.path().join("im_uploads"));

        // 灌一条已上传的文件，拿 id
        let rec = upload_store
            .put(b"hello", Some("Screenshot.png"), Some("image/png"), None)
            .await
            .expect("put");

        // 写一条 user text 消息，attachments 携 [rec.id]
        let conv_id = conv.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        conv.append_message(
            &conv_id,
            "user",
            None,
            "text",
            &serde_json::json!({"text": "看图"}),
            Some(&serde_json::json!([rec.id])),
            None,
            Utc::now(),
        )
        .await
        .unwrap();

        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi)
            .with_conv_store(conv)
            .with_upload_store(upload_store);
        let app = Router::new()
            .route("/api/conv/messages", axum_get(super::messages))
            .with_state(state);

        let resp = app
            .oneshot(req("/api/conv/messages?conv=xuannv&limit=20"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let msgs = parsed["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        let m = &msgs[0];
        // 老的 attachments 仍是 string[] (id 列表)，新的 attachment_uploads 含真名
        let atts = m["attachments"].as_array().unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].as_str().unwrap(), rec.id);
        let hydrated = m["attachment_uploads"].as_array().unwrap();
        assert_eq!(hydrated.len(), 1, "应 hydrate 出 1 条 upload meta");
        assert_eq!(hydrated[0]["id"].as_str().unwrap(), rec.id);
        assert_eq!(
            hydrated[0]["name"].as_str().unwrap(),
            "Screenshot.png",
            "name 应是原始文件名而非 uuid"
        );
        assert_eq!(hydrated[0]["mime"].as_str().unwrap(), "image/png");
    }

    /// 没注入 upload_store 时 handler 不报错，attachment_uploads 字段缺失（前端 fallback）。
    #[tokio::test]
    async fn no_upload_store_skips_hydration_gracefully() {
        let (_dir, app, store) = build_app().await;
        let conv_id = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        store
            .append_message(
                &conv_id,
                "user",
                None,
                "text",
                &serde_json::json!({"text": "hi"}),
                Some(&serde_json::json!(["fake-id"])),
                None,
                Utc::now(),
            )
            .await
            .unwrap();
        let resp = app
            .oneshot(req("/api/conv/messages?conv=xuannv&limit=20"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let m = &parsed["messages"].as_array().unwrap()[0];
        // 没 hydrate → 字段省略（serde skip_serializing_if Vec::is_empty）
        assert!(
            m.get("attachment_uploads").is_none()
                || m["attachment_uploads"]
                    .as_array()
                    .map(|a| a.is_empty())
                    .unwrap_or(true),
            "无 upload_store 时应不带 attachment_uploads 或空数组"
        );
    }

    #[test]
    fn conv_visible_only_current_topic_clone_events() {
        use fuxi_core::TopicId;
        use fuxi_core::event::{EventKind, EventMeta};
        use fuxi_core::id::AgentId;
        use std::collections::HashMap;

        let topic_a = TopicId::new();
        let topic_b = TopicId::new();
        let clone_a = AgentId::new();
        let clone_b = AgentId::new();
        let pool: HashMap<TopicId, AgentId> = [(topic_a, clone_a), (topic_b, clone_b)].into();

        let ev = |agent: Option<AgentId>| {
            let mut meta = EventMeta::now();
            meta.agent = agent;
            fuxi_core::event::Event {
                meta,
                kind: EventKind::AgentResponded {
                    text: "hi".into(),
                    artifact_ref: None,
                },
            }
        };

        // 当前 topic = A：A 分身可见，B 分身不可见（落库走小红点），门客/平台级不可见
        assert!(conv_event_visible(&pool, topic_a, &ev(Some(clone_a))));
        assert!(!conv_event_visible(&pool, topic_a, &ev(Some(clone_b))));
        assert!(!conv_event_visible(
            &pool,
            topic_a,
            &ev(Some(AgentId::new()))
        ));
        assert!(!conv_event_visible(&pool, topic_a, &ev(None)));
        // 切到 B 后跟随
        assert!(conv_event_visible(&pool, topic_b, &ev(Some(clone_b))));
    }

    #[tokio::test]
    async fn returns_503_when_store_not_injected() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (_ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi); // 默认无 conv_store
        let app = Router::new()
            .route("/api/conv/messages", axum_get(super::messages))
            .with_state(state);
        let resp = app.oneshot(req("/api/conv/messages")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
