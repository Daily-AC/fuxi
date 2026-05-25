//! γ · WebSocket 事件流 + 历史 replay 端到端测。
//!
//! 起真实 axum server（ephemeral port）+ 真实 WS 客户端（tokio-tungstenite）+ 真实
//! `EventBus`，验证：
//!   1. `WS /api/conv` 订阅玄女事件，publish 后能在客户端 deserialize 成 `EventKind`
//!   2. `WS /api/conv?from=<id>` cursor replay：先发 N 条历史，再连 + tail live
//!   3. `WS /api/tasks/{id}/stream` 按 task_id 过滤——订 A 不收 B
//!   4. broadcast Lagged 不挂订阅链——海量发布 → server 不崩、客户端继续可读
//!   5. `EventKind` 联合 round-trip serde（防 wire format 退化——下发即原 enum）
//!   6. `GET /api/tasks/{id}/events?from=<cursor>&limit=N` 历史回放分页
//!
//! 为什么这样写：firehose `tests/hub_roundtrip.rs` 同款思路——单元测 hook 不到 axum
//! 路由 + 真 socket，唯有起 server 才能暴露 frame 边界/查询参数解析坑。

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskId};
use fuxi_events::EventBus;
use fuxi_im::auth::{COOKIE_NAME, HmacSecret, fresh_claims, sign_token};
use fuxi_im::devices::DeviceStore;
use fuxi_im::pair::PendingPairs;
use fuxi_im::state::ImAuth;
use fuxi_im::{AppState, router};
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::GitWorktreeWorkspace;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tower::ServiceExt;
use url::Url;

async fn make_workspace() -> (TempDir, Arc<GitWorktreeWorkspace>) {
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
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .await
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?} failed");
}

/// 启 IM server——返回 base url、bus、xuannv_id（已注入）、server handle 和
/// 一个合法 cookie 字符串（`fuxi_im_token=...`）。`/api/*` 调用都要带它。
///
/// #15 之后：router 强制鉴权，所有真 server e2e 测试必须带 cookie。注入显式
/// secret + 同 secret 签 token 是测试 helper 的固定模式。
async fn spawn_im() -> (
    String,
    EventBus,
    AgentId,
    String,
    TempDir,
    tokio::task::JoinHandle<()>,
) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus.clone(), ws));

    // 玄女 id：测试不真起 cc，只挂个 id 让 handler 拿得到。
    let xuannv_id = AgentId::new();
    fuxi.set_xuannv(xuannv_id).await;

    let secret = HmacSecret::from_string("ws-test-key".into());
    let secret_arc = Arc::new(secret);
    let im_auth = ImAuth {
        secret: secret_arc.clone(),
        pairs: Arc::new(PendingPairs::new()),
        devices: None::<DeviceStore>,
        password_path: None,
        login_guard: Arc::new(fuxi_im::lockout::LoginGuard::new()),
    };
    let state = AppState::new(fuxi).with_im_auth(im_auth);
    let app = router(state);

    let claims = fresh_claims("ws-test-device".into(), "ws-test".into());
    let token = sign_token(&secret_arc, &claims).expect("sign");
    let cookie = format!("{COOKIE_NAME}={token}");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let h = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let base = format!("http://{addr}");
    (base, bus, xuannv_id, cookie, dir, h)
}

/// 用合法 cookie 建 WebSocket 连接——所有 `/api/conv` `/api/tasks/.../stream`
/// 测试都过这个 helper。
async fn ws_connect_with_cookie(
    url: &str,
    cookie: &str,
) -> (
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
) {
    let mut req = url.into_client_request().expect("ws into client request");
    req.headers_mut().insert(
        "Cookie",
        tokio_tungstenite::tungstenite::http::HeaderValue::from_str(cookie)
            .expect("cookie header value"),
    );
    connect_async(req).await.expect("ws connect")
}

fn agent_event(agent: AgentId, text: &str) -> Event {
    let mut meta = EventMeta::now();
    meta.agent = Some(agent);
    Event {
        meta,
        kind: EventKind::AgentResponded {
            text: text.into(),
            artifact_ref: None,
        },
    }
}

/// P0.D #45 修后 `/api/{tasks,workers}/{id}/events` HTTP 响应 wire shape =
/// `{events, next_cursor}`（对齐前端 `EventHistoryResponse`）。e2e 测都
/// 走该 helper 抽 `events` 字段、顺带断言 `next_cursor` 当前为 null。
async fn read_events_resp(resp: reqwest::Response) -> Vec<Event> {
    let body: serde_json::Value = resp.json().await.expect("json");
    assert!(
        body.get("next_cursor").is_some_and(|v| v.is_null()),
        "EventHistoryResponse.next_cursor 当前应为 null（无服务端分页），实际: {body}"
    );
    let evs_val = body
        .get("events")
        .cloned()
        .expect("EventHistoryResponse 应有 .events 数组字段");
    serde_json::from_value::<Vec<Event>>(evs_val).expect("events 反序列化")
}

/// 单任务白名单事件——用 `AgentResponded` 携带 label 文本，既满足 #N6'
/// `task_thread_visible` 白名单（`/api/tasks/:id/{events,conv}` 走该 filter），也
/// 让旧 `/api/tasks/:id/stream`（裸 task_id 过滤，无白名单）继续兼容。
///
/// 历史上这里用 `EventKind::Custom`，但 #N6' 上线后该 kind 不在 task thread 白
/// 名单内——会被 `/events` HTTP 端 filter 滤掉，导致 e2e 假阴性。
fn task_event(task: TaskId, label: &str) -> Event {
    let mut meta = EventMeta::now();
    meta.task = Some(task);
    Event {
        meta,
        kind: EventKind::AgentResponded {
            text: label.into(),
            artifact_ref: None,
        },
    }
}

async fn next_event(
    stream: &mut (
             impl StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
             + Unpin
         ),
    label: &str,
) -> Event {
    loop {
        let msg = match tokio::time::timeout(Duration::from_secs(3), stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(e))) => panic!("[{label}] ws err: {e}"),
            Ok(None) => panic!("[{label}] 流关闭"),
            Err(_) => panic!("[{label}] 等待 3s 没收到帧"),
        };
        match msg {
            Message::Text(t) => {
                return serde_json::from_str::<Event>(&t)
                    .unwrap_or_else(|e| panic!("[{label}] event 反序列化失败: {e}, body={t}"));
            }
            // 心跳/控制帧忽略，直到拿到下一条 text。
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            Message::Binary(_) => continue,
            Message::Close(_) => panic!("[{label}] server 主动 close"),
        }
    }
}

#[tokio::test]
async fn ws_conv_streams_xuannv_events_live() {
    let (base, bus, xuannv, cookie, _dir, _srv) = spawn_im().await;
    let url = Url::parse(&(base.replace("http://", "ws://") + "/api/conv")).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();

    // 给 server 一点时间订上 broadcast——不是轮询，是握手延迟。
    tokio::time::sleep(Duration::from_millis(120)).await;

    bus.publish(agent_event(xuannv, "你好以琳"))
        .expect("publish");

    let got = next_event(&mut r, "conv-live").await;
    match got.kind {
        EventKind::AgentResponded { text, .. } => assert_eq!(text, "你好以琳"),
        other => panic!("expect AgentResponded, got {other:?}"),
    }
}

/// Jarvis · 反回归：`XuannvVoiceLine` 事件 publish 时只要 `meta.agent=xuannv_id`，
/// `/api/conv` WS 必须透传给客户端（macOS App 靠这条事件触发 TTS）。
/// 守现有"按 agent 过滤、不维护 EventKind 白名单"的设计——加新 EventKind 不该改 conv。
#[tokio::test]
async fn ws_conv_streams_xuannv_voice_line() {
    let (base, bus, xuannv, cookie, _dir, _srv) = spawn_im().await;
    let url = Url::parse(&(base.replace("http://", "ws://") + "/api/conv")).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();

    tokio::time::sleep(Duration::from_millis(120)).await;

    let mut meta = EventMeta::now();
    meta.agent = Some(xuannv);
    bus.publish(Event {
        meta,
        kind: EventKind::XuannvVoiceLine {
            text: "好的，已派给鲁班".into(),
            emotion: None,
        },
    })
    .expect("publish");

    let got = next_event(&mut r, "conv-voice-line").await;
    match got.kind {
        EventKind::XuannvVoiceLine { text, .. } => assert_eq!(text, "好的，已派给鲁班"),
        other => panic!("expect XuannvVoiceLine, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_conv_filters_out_other_agents() {
    let (base, bus, xuannv, cookie, _dir, _srv) = spawn_im().await;
    let url = Url::parse(&(base.replace("http://", "ws://") + "/api/conv")).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();

    tokio::time::sleep(Duration::from_millis(120)).await;

    let other = AgentId::new();
    // 噪声：别的门客的事件——不是玄女 id，应该被过滤掉。
    bus.publish(agent_event(other, "门客噪音"))
        .expect("publish");
    // 真信号：玄女说话。
    bus.publish(agent_event(xuannv, "玄女发言"))
        .expect("publish");

    let got = next_event(&mut r, "conv-filter").await;
    match got.kind {
        EventKind::AgentResponded { text, .. } => assert_eq!(text, "玄女发言", "应只收玄女事件"),
        other => panic!("expect AgentResponded, got {other:?}"),
    }
}

#[tokio::test]
async fn ws_conv_replay_from_cursor_then_tails_live() {
    let (base, bus, xuannv, cookie, _dir, _srv) = spawn_im().await;

    // 先发三条历史；anchor = a。
    let a = agent_event(xuannv, "a");
    let b = agent_event(xuannv, "b");
    let c = agent_event(xuannv, "c");
    let anchor = a.meta.id;
    bus.publish(a).expect("publish a");
    bus.publish(b).expect("publish b");
    bus.publish(c).expect("publish c");
    // 等 writer flush 进 SQLite。
    tokio::time::sleep(Duration::from_millis(220)).await;

    let url_s = format!(
        "{}/api/conv?from={}",
        base.replace("http://", "ws://"),
        anchor
    );
    let url = Url::parse(&url_s).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();

    // 应严格在 a 之后——先 b、再 c。
    let first = next_event(&mut r, "replay-1").await;
    let second = next_event(&mut r, "replay-2").await;
    match first.kind {
        EventKind::AgentResponded { text, .. } => assert_eq!(text, "b"),
        o => panic!("expect b, got {o:?}"),
    }
    match second.kind {
        EventKind::AgentResponded { text, .. } => assert_eq!(text, "c"),
        o => panic!("expect c, got {o:?}"),
    }

    // 再发一条 live d——live tail 必须连上来。
    tokio::time::sleep(Duration::from_millis(120)).await;
    bus.publish(agent_event(xuannv, "d")).expect("publish d");
    let third = next_event(&mut r, "replay-tail").await;
    match third.kind {
        EventKind::AgentResponded { text, .. } => assert_eq!(text, "d"),
        o => panic!("expect d, got {o:?}"),
    }
}

#[tokio::test]
async fn ws_task_stream_filters_by_task_id() {
    let (base, bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;

    let task_a = TaskId::new();
    let task_b = TaskId::new();

    let url_s = format!(
        "{}/api/tasks/{}/stream",
        base.replace("http://", "ws://"),
        task_a
    );
    let url = Url::parse(&url_s).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();
    tokio::time::sleep(Duration::from_millis(120)).await;

    // 先发 task_b 的噪声，再发 task_a 的真信号——订阅 A 应只收到 A。
    bus.publish(task_event(task_b, "B-noise"))
        .expect("publish B");
    bus.publish(task_event(task_a, "A-signal"))
        .expect("publish A");

    let got = next_event(&mut r, "task-filter").await;
    match got.kind {
        EventKind::AgentResponded { text, .. } => {
            assert_eq!(text, "A-signal", "应只收 task_a 事件")
        }
        o => panic!("expect AgentResponded, got {o:?}"),
    }
    // meta.task 必须是 task_a。
    assert_eq!(got.meta.task, Some(task_a));
}

#[tokio::test]
async fn ws_conv_survives_broadcast_lag() {
    // broadcast Lagged 不挂订阅链——CLAUDE.md 强调的坑。
    // 做法：扔大量事件 → server WS loop 应继续运行、客户端能继续读到后续事件。
    let (base, bus, xuannv, cookie, _dir, _srv) = spawn_im().await;
    let url = Url::parse(&(base.replace("http://", "ws://") + "/api/conv")).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();
    tokio::time::sleep(Duration::from_millis(120)).await;

    // 灌大量事件——broadcast 容量 1024，发 4000 条触发 Lagged。
    for i in 0..4000 {
        bus.publish(agent_event(xuannv, &format!("flood-{i}")))
            .expect("publish");
    }
    // 客户端来不及消费的事件被 broadcast 端 lag 掉，但 server WS loop 不应退出。
    // 留充裕窗口让 broadcast 把队列消化掉。
    tokio::time::sleep(Duration::from_millis(400)).await;

    // 再发一条新事件——若 server 挂了、订阅死了，这条收不到；活的话能收到。
    bus.publish(agent_event(xuannv, "after-lag"))
        .expect("publish after");

    // 跳过 lag 期间能拿到的所有 flood-*，找到 after-lag。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("server 未在 broadcast lag 后存活——没收到 after-lag 事件");
        }
        let msg = tokio::time::timeout(Duration::from_secs(2), r.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        if let Message::Text(t) = msg {
            let ev: Event = serde_json::from_str(&t).expect("event");
            if let EventKind::AgentResponded { text, .. } = ev.kind
                && text == "after-lag"
            {
                break;
            }
        }
    }
}

#[tokio::test]
async fn ws_event_kind_serde_roundtrip_preserves_tag_union() {
    // 防 wire format 退化：服务端下发的就是 `EventKind` 的 `#[serde(tag="type")]`，
    // 客户端拿到 JSON 反序列化回 `Event` 必须 OK，且 type tag 是 snake_case。
    let (base, bus, xuannv, cookie, _dir, _srv) = spawn_im().await;
    let url = Url::parse(&(base.replace("http://", "ws://") + "/api/conv")).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();
    tokio::time::sleep(Duration::from_millis(120)).await;

    let mut meta = EventMeta::now();
    meta.agent = Some(xuannv);
    let ev = Event {
        meta,
        kind: EventKind::ToolCallStarted {
            tool: "Bash".into(),
            args: serde_json::json!({"command": "ls"}),
        },
    };
    bus.publish(ev).expect("publish");

    // 拿到原始 text，先做"裸 JSON 形状"断言（type tag = snake_case），再走 serde。
    let raw = loop {
        let msg = tokio::time::timeout(Duration::from_secs(3), r.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        match msg {
            Message::Text(t) => break t,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            other => panic!("expect text, got {other:?}"),
        }
    };
    let raw_str: String = raw.to_string();
    let v: serde_json::Value = serde_json::from_str(&raw_str).expect("json");
    let kind_tag = v
        .get("kind")
        .and_then(|k| k.get("type"))
        .and_then(|t| t.as_str())
        .expect("kind.type 应在");
    assert_eq!(
        kind_tag, "tool_call_started",
        "wire 上 EventKind 标签必须是 snake_case"
    );

    // 完整 round-trip。
    let parsed: Event = serde_json::from_str(&raw_str).expect("Event roundtrip");
    match parsed.kind {
        EventKind::ToolCallStarted { tool, args } => {
            assert_eq!(tool, "Bash");
            assert_eq!(args["command"], "ls");
        }
        other => panic!("expect ToolCallStarted, got {other:?}"),
    }
}

#[tokio::test]
async fn http_task_events_returns_history_with_pagination() {
    let (base, bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;
    let task = TaskId::new();

    // 灌 5 条 task 事件 + 1 条噪声（不同 task）。
    for i in 0..5 {
        bus.publish(task_event(task, &format!("h-{i}")))
            .expect("publish");
    }
    bus.publish(task_event(TaskId::new(), "other-task"))
        .expect("publish noise");
    // 等 SQLite flush。
    tokio::time::sleep(Duration::from_millis(220)).await;

    let http = reqwest::Client::new();
    // 默认（无 from）：返全部 5 条该 task 历史，限 limit=3。
    let url = format!("{base}/api/tasks/{task}/events?limit=3");
    let resp = http
        .get(&url)
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let evs = read_events_resp(resp).await;
    assert_eq!(evs.len(), 3, "limit=3 应只返 3 条");
    for e in &evs {
        assert_eq!(e.meta.task, Some(task), "必须是该 task 事件");
    }
    // 顺序：h-0, h-1, h-2（按 rowid 升序）。
    let labels: Vec<String> = evs
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::AgentResponded { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["h-0", "h-1", "h-2"]);
}

#[tokio::test]
async fn http_task_events_with_cursor_returns_strictly_after() {
    let (base, bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;
    let task = TaskId::new();

    let a = task_event(task, "a");
    let b = task_event(task, "b");
    let c = task_event(task, "c");
    let anchor = a.meta.id;
    bus.publish(a).expect("publish a");
    bus.publish(b).expect("publish b");
    bus.publish(c).expect("publish c");
    tokio::time::sleep(Duration::from_millis(220)).await;

    let http = reqwest::Client::new();
    let url = format!("{base}/api/tasks/{task}/events?from={anchor}");
    let resp = http
        .get(&url)
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let evs = read_events_resp(resp).await;
    assert_eq!(evs.len(), 2, "anchor 之后 2 条");
    let labels: Vec<String> = evs
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::AgentResponded { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["b", "c"]);
}

/// #N6' 新端点：`WS /api/tasks/:id/conv`——任务 thread 流式接续。
/// 镜像 `/api/tasks/:id/stream` 的 task_id 过滤逻辑，但走白名单 EventKind
/// （`task_thread_visible`）。本测验"task_b 噪声不进 task_a thread"。
#[tokio::test]
async fn ws_task_conv_filters_by_task_id() {
    let (base, bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;

    let task_a = TaskId::new();
    let task_b = TaskId::new();

    let url_s = format!(
        "{}/api/tasks/{}/conv",
        base.replace("http://", "ws://"),
        task_a
    );
    let url = Url::parse(&url_s).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();
    tokio::time::sleep(Duration::from_millis(120)).await;

    bus.publish(task_event(task_b, "B-noise"))
        .expect("publish B");
    bus.publish(task_event(task_a, "A-signal"))
        .expect("publish A");

    let got = next_event(&mut r, "task-conv-filter").await;
    match got.kind {
        EventKind::AgentResponded { text, .. } => {
            assert_eq!(text, "A-signal", "应只收 task_a 事件")
        }
        o => panic!("expect AgentResponded, got {o:?}"),
    }
    assert_eq!(got.meta.task, Some(task_a));
}

/// #N6' 白名单：`Custom`/`AgentSpawning`/`TaskCreated` 等"非对话内容"事件
/// 不进 `/events` HTTP 响应——前端任务 thread 只渲染 `task_thread_visible`。
#[tokio::test]
async fn http_task_events_drops_non_whitelisted_kinds() {
    let (base, bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;
    let task = TaskId::new();

    // 白名单内：AgentResponded
    bus.publish(task_event(task, "say")).expect("publish text");
    // 白名单外：Custom（早期任务级原始事件流用过）
    let mut meta = EventMeta::now();
    meta.task = Some(task);
    bus.publish(Event {
        meta,
        kind: EventKind::Custom {
            label: "platform-noise".into(),
            payload: serde_json::json!({}),
        },
    })
    .expect("publish custom");
    // 白名单外：TaskCreated
    let mut meta2 = EventMeta::now();
    meta2.task = Some(task);
    bus.publish(Event {
        meta: meta2,
        kind: EventKind::TaskCreated {
            title: "T".into(),
            description: "".into(),
        },
    })
    .expect("publish created");
    tokio::time::sleep(Duration::from_millis(220)).await;

    let http = reqwest::Client::new();
    let url = format!("{base}/api/tasks/{task}/events");
    let resp = http
        .get(&url)
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let evs = read_events_resp(resp).await;
    assert_eq!(evs.len(), 1, "白名单外 2 条应被过滤，只剩 AgentResponded");
    match &evs[0].kind {
        EventKind::AgentResponded { text, .. } => assert_eq!(text, "say"),
        o => panic!("expect AgentResponded, got {o:?}"),
    }
}

#[tokio::test]
async fn http_task_events_rejects_bad_cursor() {
    let (base, _bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;
    let task = TaskId::new();
    let http = reqwest::Client::new();
    let url = format!("{base}/api/tasks/{task}/events?from=not-a-uuid");
    let resp = http
        .get(&url)
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("get");
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

/// `/api/nodes/stream` 实时推 WorkerHeartbeatStateChanged 给前端 NodesPage——
/// 验过滤白名单：节点级三类事件下发，agent 级事件不下发。
#[tokio::test]
async fn ws_nodes_stream_pushes_worker_heartbeat_events() {
    use fuxi_core::WorkerStatus;
    let (base, bus, _xuannv, cookie, _dir, _srv) = spawn_im().await;
    let url = Url::parse(&(base.replace("http://", "ws://") + "/api/nodes/stream")).expect("parse");
    let (ws, _) = ws_connect_with_cookie(url.as_str(), &cookie).await;
    let (_w, mut r) = ws.split();
    tokio::time::sleep(Duration::from_millis(120)).await;

    // 噪音：玄女发言事件——非节点级，应被 filter 掉
    bus.publish(agent_event(AgentId::new(), "应被过滤"))
        .expect("publish noise");
    // 真信号：worker 心跳状态变化
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::WorkerHeartbeatStateChanged {
            node_id: "home".into(),
            inflight_count: 1,
            status: WorkerStatus::Alive,
        },
    })
    .expect("publish hb");

    let got = next_event(&mut r, "nodes-stream").await;
    match got.kind {
        EventKind::WorkerHeartbeatStateChanged {
            node_id,
            inflight_count,
            ..
        } => {
            assert_eq!(node_id, "home");
            assert_eq!(inflight_count, 1);
        }
        other => panic!("expect WorkerHeartbeatStateChanged, got {other:?}"),
    }
}

/// 路由级冒烟：α 留的 `/api/tasks` 不动；γ 不该破坏它。
#[tokio::test]
async fn list_tasks_root_still_returns_array() {
    // 注：spawn_im() 在这里只是历史习惯——不需要起真 server，本测用 oneshot 即可。
    // 单独起 router + 注入 secret + 手签 cookie 验"layer 不拦合法请求 + handler 返数组"。
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (_dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));
    let secret = HmacSecret::from_string("oneshot-key".into());
    let secret_arc = Arc::new(secret);
    let im_auth = ImAuth {
        secret: secret_arc.clone(),
        pairs: Arc::new(PendingPairs::new()),
        devices: None::<DeviceStore>,
        password_path: None,
        login_guard: Arc::new(fuxi_im::lockout::LoginGuard::new()),
    };
    let app = router(AppState::new(fuxi).with_im_auth(im_auth));
    let claims = fresh_claims("oneshot-device".into(), "oneshot".into());
    let token = sign_token(&secret_arc, &claims).expect("sign");
    let cookie = format!("{COOKIE_NAME}={token}");

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/tasks?root=1")
                .header(axum::http::header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // β · #21 后契约：`{ running, completed }` object（之前 α stub 返空数组）
    assert!(v.is_object());
    assert!(v["running"].is_array());
    assert!(v["completed"].is_array());
}
