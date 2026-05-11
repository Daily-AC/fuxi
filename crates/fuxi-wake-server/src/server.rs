//! axum app + WS handler。
//!
//! `/api/wake`（WS）：升级前 check `Authorization: Bearer <token>`；连上后跑
//! `run_wake_loop`——多路 select：客户端帧 / 引擎喂音频后回返 wake / 5s ping。
//!
//! `/health`：GET 200 文本，给 systemd / 探活检查。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use chrono::Utc;
use std::net::SocketAddr;
use tracing::{debug, error, info, warn};

use crate::auth;
use crate::engine::WakeEngine;
use crate::protocol::{ClientMessage, ServerMessage};

/// 服务端心跳间隔（与协议 5s 对齐）。
pub const PING_INTERVAL: Duration = Duration::from_secs(5);

/// 入站静默上限——15s 内没收到任何上行帧就关连接（协议契约）。
pub const INBOUND_IDLE_LIMIT: Duration = Duration::from_secs(15);

/// 应用状态：token + 引擎工厂 + awake 计数（health 用）。
///
/// 引擎工厂：每个连接拿一个独立 engine 实例（讯飞 SDK 通常是单 session 设计）。
pub struct AppState {
    pub token: String,
    pub engine_factory: Box<dyn Fn() -> Box<dyn WakeEngine> + Send + Sync>,
    pub awake_count: AtomicU64,
    pub sdk_status: parking_lot_lite::Atomic,
}

/// 极简 atomic 字符串状态——避免引入 parking_lot/dashmap 重依赖。
mod parking_lot_lite {
    use std::sync::atomic::{AtomicU8, Ordering};

    /// SDK 状态枚举：ready / degraded / down → u8。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Status {
        Ready,
        Degraded,
        Down,
    }

    impl Status {
        pub fn as_str(self) -> &'static str {
            match self {
                Status::Ready => "ready",
                Status::Degraded => "degraded",
                Status::Down => "down",
            }
        }
        fn to_u8(self) -> u8 {
            match self {
                Status::Ready => 0,
                Status::Degraded => 1,
                Status::Down => 2,
            }
        }
        fn from_u8(v: u8) -> Self {
            match v {
                0 => Status::Ready,
                1 => Status::Degraded,
                _ => Status::Down,
            }
        }
    }

    pub struct Atomic(AtomicU8);
    impl Atomic {
        pub fn new(s: Status) -> Self {
            Self(AtomicU8::new(s.to_u8()))
        }
        pub fn load(&self) -> Status {
            Status::from_u8(self.0.load(Ordering::Relaxed))
        }
        pub fn store(&self, s: Status) {
            self.0.store(s.to_u8(), Ordering::Relaxed);
        }
    }
}

pub use parking_lot_lite::Status as SdkStatus;

impl AppState {
    pub fn new<F>(token: String, engine_factory: F) -> Self
    where
        F: Fn() -> Box<dyn WakeEngine> + Send + Sync + 'static,
    {
        Self {
            token,
            engine_factory: Box::new(engine_factory),
            awake_count: AtomicU64::new(0),
            sdk_status: parking_lot_lite::Atomic::new(SdkStatus::Ready),
        }
    }
}

/// 构造 axum Router。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/wake", any(wake_ws))
        .route("/health", get(health))
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let body = serde_json::json!({
        "status": "ok",
        "sdk": state.sdk_status.load().as_str(),
        "awake_count": state.awake_count.load(Ordering::Relaxed),
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// WS 升级入口——Authorization 校验失败直接 401。
///
/// 两条 token 通道（两条用同一颗预共享 `~/.fuxi/wake.token` 比对，安全等价）：
/// - **Authorization: Bearer <token>**：药丸 v0.2 走的；Swift URLRequest 加 header
/// - **Query `?token=<token>`**：桌宠 Tauri webview 走的；Web WebSocket API 不能
///   set custom header，token 只能塞 URL
async fn wake_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    let header_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(auth::parse_bearer)
        .map(str::to_string);
    let query_token = q
        .get("token")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(token) = header_token.or(query_token) else {
        warn!(%addr, "wake ws: 缺 token（Authorization Bearer 或 ?token=）");
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };
    if !auth::constant_time_eq(&token, &state.token) {
        warn!(%addr, "wake ws: token 不匹配");
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }

    // 取 token 前 8 位做 client_id，方便日志追溯（不打全 token）。
    let client_id = token.chars().take(8).collect::<String>();
    info!(%addr, %client_id, "wake ws: upgrade ok");

    let engine = (state.engine_factory)();
    ws.on_upgrade(move |socket| run_wake_loop(socket, engine, state, client_id))
}

/// WS 主循环——一连接一 loop。
async fn run_wake_loop(
    socket: WebSocket,
    engine: Box<dyn WakeEngine>,
    state: Arc<AppState>,
    client_id: String,
) {
    if let Err(e) = run_wake_loop_inner(socket, engine.as_ref(), &state, &client_id).await {
        error!(%client_id, error = ?e, "wake ws: loop 异常退出");
    }
    if let Err(e) = engine.close().await {
        warn!(%client_id, error = ?e, "wake ws: engine close 报错");
    }
}

async fn run_wake_loop_inner(
    mut socket: WebSocket,
    engine: &dyn WakeEngine,
    state: &Arc<AppState>,
    client_id: &str,
) -> anyhow::Result<()> {
    // 第一帧必须是 hello——握手带超时。
    let hello = match tokio::time::timeout(Duration::from_secs(10), socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => t,
        Ok(Some(Ok(other))) => {
            warn!(%client_id, ?other, "wake ws: 期望 hello，收到非文本帧");
            send_error(
                &mut socket,
                "audio_format_invalid",
                "first frame must be hello",
            )
            .await;
            return Ok(());
        }
        Ok(Some(Err(e))) => {
            warn!(%client_id, error = ?e, "wake ws: 入站错误");
            return Ok(());
        }
        Ok(None) => return Ok(()),
        Err(_) => {
            warn!(%client_id, "wake ws: hello 超时");
            return Ok(());
        }
    };

    let parsed: Result<ClientMessage, _> = serde_json::from_str(&hello);
    let keywords = match parsed {
        Ok(ClientMessage::Hello { client, version }) => {
            info!(%client_id, %client, %version, "wake ws: hello");
            vec!["玄女".to_string()]
        }
        Ok(other) => {
            warn!(%client_id, ?other, "wake ws: 期望 hello，收到 {other:?}");
            send_error(&mut socket, "audio_format_invalid", "expected hello first").await;
            return Ok(());
        }
        Err(e) => {
            warn!(%client_id, error = %e, raw = %hello, "wake ws: hello JSON 解析失败");
            send_error(&mut socket, "audio_format_invalid", "bad hello json").await;
            return Ok(());
        }
    };

    if let Err(e) = engine.init(&keywords).await {
        warn!(%client_id, error = ?e, "wake ws: engine init 失败");
        send_error(&mut socket, "sdk_unavailable", &e.to_string()).await;
        state.sdk_status.store(SdkStatus::Down);
        return Ok(());
    }
    state.sdk_status.store(SdkStatus::Ready);

    // ready 下发。
    let ready = ServerMessage::Ready {
        keywords: keywords.clone(),
    };
    let ready_json = serde_json::to_string(&ready)?;
    if socket.send(Message::Text(ready_json.into())).await.is_err() {
        return Ok(());
    }

    // 心跳定时器——首次 tick 立即就绪，跳过让客户端先发音频几秒后再 ping。
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;

    let mut last_inbound = tokio::time::Instant::now();
    let mut idle_check = tokio::time::interval(Duration::from_secs(1));
    idle_check.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = ping.tick() => {
                let p = ServerMessage::Ping { at: Utc::now() };
                let s = serde_json::to_string(&p)?;
                if socket.send(Message::Text(s.into())).await.is_err() {
                    debug!(%client_id, "wake ws: ping 失败，peer 断");
                    break;
                }
            }
            _ = idle_check.tick() => {
                if last_inbound.elapsed() > INBOUND_IDLE_LIMIT {
                    warn!(%client_id, "wake ws: 入站静默 > 15s，关连接");
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1000,
                            reason: "idle".into(),
                        })))
                        .await;
                    break;
                }
            }
            incoming = socket.recv() => {
                let msg = match incoming {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        warn!(%client_id, error = ?e, "wake ws: 入站错误");
                        break;
                    }
                    None => {
                        debug!(%client_id, "wake ws: peer 关流");
                        break;
                    }
                };
                last_inbound = tokio::time::Instant::now();
                match msg {
                    Message::Binary(pcm) => {
                        match engine.feed(&pcm).await {
                            Ok(Some((keyword, score))) => {
                                state.awake_count.fetch_add(1, Ordering::Relaxed);
                                info!(%client_id, %keyword, %score, "wake ws: 命中");
                                let evt = ServerMessage::Wake { keyword, score, at: Utc::now() };
                                let s = serde_json::to_string(&evt)?;
                                if socket.send(Message::Text(s.into())).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(%client_id, error = ?e, "wake ws: engine feed 失败");
                                send_error(&mut socket, "sdk_unavailable", &e.to_string()).await;
                                break;
                            }
                        }
                    }
                    Message::Text(t) => {
                        match serde_json::from_str::<ClientMessage>(&t) {
                            Ok(ClientMessage::Pong { .. }) => {}
                            Ok(ClientMessage::Bye) => {
                                info!(%client_id, "wake ws: client bye");
                                break;
                            }
                            Ok(ClientMessage::Keywords { .. }) => {
                                // v0.1 不实现切换；静默忽略。
                            }
                            Ok(ClientMessage::Hello { .. }) => {
                                warn!(%client_id, "wake ws: 重复 hello");
                            }
                            Err(e) => {
                                warn!(%client_id, error = %e, raw = %t, "wake ws: 文本帧 JSON 解析失败");
                            }
                        }
                    }
                    Message::Ping(_) | Message::Pong(_) => {
                        // tungstenite/axum 内部已自动 pong，这里收到无需额外处理。
                    }
                    Message::Close(_) => {
                        debug!(%client_id, "wake ws: client close");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn send_error(socket: &mut WebSocket, code: &str, message: &str) {
    let m = ServerMessage::Error {
        code: code.into(),
        message: message.into(),
    };
    if let Ok(s) = serde_json::to_string(&m) {
        let _ = socket.send(Message::Text(s.into())).await;
    }
}
