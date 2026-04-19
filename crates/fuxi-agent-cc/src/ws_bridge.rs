//! Claude Code `--sdk-url` 反连 WebSocket 桥。
//!
//! v0.1 薄片 H：替代原本的 stdio `--print` 路径。设计参照 team-anya
//! `apps/server/src/broker/backends/claude-code-backend.ts`（1066 行 TS）
//! 的 `setupMessageHandler` / `sendToCLI` / `routeMessage` 逻辑。
//!
//! ## 为什么是「反连」
//!
//! Claude Code CLI 以 `--sdk-url ws://127.0.0.1:<port>/ws/cli/<sid>` 启动时
//! 会主动把这个地址当成"我要反连到它"，换句话说：**CLI 是 WS client，
//! 我们是 WS server**。好处：
//!
//! 1. NDJSON 双向对称——发 prompt / 收事件都是 `ws.send / ws.recv`，
//!    不用 stdin/stdout 两条半双工管子拼；
//! 2. 拿到 `control_request` 通道——可以**打断 turn**（`subtype:"interrupt"`）
//!    和响应**工具审批**（`subtype:"can_use_tool"`）；
//! 3. 拿到 `tool_progress` / `keep_alive` / `rate_limit_event` 这些 stdio
//!    模式没有的细粒度信号；
//! 4. CLI 一侧即使进了 hung loop，我们仍然能发 interrupt——stdio 模式
//!    下唯一出路是 SIGINT 杀进程。
//!
//! ## 启动时序
//!
//! ```text
//! 1. WsChannel::bind(sid) ── 起 axum server @ 127.0.0.1:<port>
//! 2. 拿到 port → 调用方用它拼 --sdk-url
//! 3. 调用方 spawn claude  ── claude 反连进来
//! 4. wait_connect().await  ── 块到 WS 建立或超时
//! 5. send() / recv_next() 双向通
//! 6. shutdown()           ── 停 server + 取消 task
//! ```

use axum::Router;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use futures_util::SinkExt;
use futures_util::stream::StreamExt;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, oneshot};

/// NDJSON frame 每条 JSON 的上限（字节）——溢出时截断并 warn。
/// 典型 cc 帧几 KB，100KB 给 rare 大 assistant 响应留余。
#[allow(dead_code)]
const MAX_FRAME_BYTES: usize = 100 * 1024;

/// `WsChannel` 的错误——桥的边界内问题。
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("connection timeout after {0:?}")]
    ConnectTimeout(Duration),
    #[error("channel closed")]
    Closed,
    #[error("ws: {0}")]
    Ws(String),
}

pub type Result<T> = std::result::Result<T, WsError>;

/// 反连 WS 的一条通讯管道。
pub struct WsChannel {
    /// OS 分配到的端口，调用方用它拼 --sdk-url。
    pub port: u16,
    /// 期望 CLI 反连进来用的路径（`/ws/cli/<uuid>`）。
    pub expected_path: String,
    /// 发给 CLI 的消息会进这条 chan，server 的写 task 转发到 WS。
    /// 未连接前的消息**排队**在 chan 里，连接建立时自动 flush。
    outgoing_tx: mpsc::UnboundedSender<Value>,
    /// 从 CLI 收到的消息（已 JSON 解析）。调用方在这里拉。
    incoming_rx: Arc<Mutex<mpsc::UnboundedReceiver<Value>>>,
    /// CLI 通过 `system/init` 通知的 session id，异步更新。
    cli_session_id: Arc<Mutex<Option<String>>>,
    /// 订阅 connected 的 rx（`wait_connect` 取用）。sender 侧 live 在 AppState 里。
    connected_rx: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    /// 关停令牌：drop 掉 ShutdownGuard → server task 感知到 → 退出。
    _shutdown: ShutdownGuard,
}

/// 持有 oneshot sender，drop 时触发关停。放在 WsChannel 里保证生命周期对齐。
struct ShutdownGuard {
    signal: Option<oneshot::Sender<()>>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.signal.take() {
            let _ = tx.send(());
        }
    }
}

#[derive(Clone)]
struct AppState {
    expected_path: String,
    incoming_tx: mpsc::UnboundedSender<Value>,
    /// take-once: 第一个反连的 CLI 拿走这条 rx 用来写。
    outgoing_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
    cli_session_id: Arc<Mutex<Option<String>>>,
    connected: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl WsChannel {
    /// 启动反连 server，返回已就绪的 channel（端口已分配但尚未等到 CLI 连接）。
    ///
    /// 调用方接下来要做的两件事：
    /// 1. 用 `channel.url(session_id)` 拼 `--sdk-url` 传给 claude
    /// 2. 调 `channel.wait_connect(timeout)` 块到 CLI 反连成功
    pub async fn bind(session_id: &str) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let port = addr.port();
        let expected_path = format!("/ws/cli/{session_id}");

        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel::<Value>();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<Value>();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        let cli_session_id = Arc::new(Mutex::new(None));

        let state = AppState {
            expected_path: expected_path.clone(),
            incoming_tx,
            outgoing_rx: Arc::new(Mutex::new(Some(outgoing_rx))),
            cli_session_id: cli_session_id.clone(),
            connected: Arc::new(Mutex::new(Some(connected_tx))),
        };

        // :sid 在 axum 0.8 里用 {sid} 语法。
        let app = Router::new()
            .route("/ws/cli/{sid}", get(ws_handler))
            .with_state(state);

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let guard = ShutdownGuard {
            signal: Some(shutdown_tx),
        };

        tokio::spawn(async move {
            let server = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
                tracing::debug!("ws_bridge: shutdown signal received");
            });
            if let Err(e) = server.await {
                tracing::warn!(error = %e, "ws_bridge: axum server exited with error");
            }
        });

        tracing::info!(port, path = %expected_path, "ws_bridge: listening, waiting for CLI reverse-connect");

        Ok(Self {
            port,
            expected_path,
            outgoing_tx,
            incoming_rx: Arc::new(Mutex::new(incoming_rx)),
            cli_session_id,
            connected_rx: Arc::new(Mutex::new(Some(connected_rx))),
            _shutdown: guard,
        })
    }

    /// 拼出 `--sdk-url` 要用的完整 WS URL。
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}{}", self.port, self.expected_path)
    }

    /// 等 CLI 反连建立；超时即报错（调用方可以杀子进程）。
    pub async fn wait_connect(&self, timeout: Duration) -> Result<()> {
        let rx_slot = self.connected_rx.lock().await.take();
        let Some(rx) = rx_slot else {
            // 已经 wait 过，直接成功
            return Ok(());
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(WsError::Closed),
            Err(_) => Err(WsError::ConnectTimeout(timeout)),
        }
    }

    /// 发送一条 JSON 消息给 CLI。连接未就绪时**排队**，自动 flush。
    pub fn send(&self, msg: Value) -> Result<()> {
        self.outgoing_tx.send(msg).map_err(|_| WsError::Closed)
    }

    /// 拉一条从 CLI 来的消息。`None` 表示通道关闭。
    pub async fn recv(&self) -> Option<Value> {
        let mut rx = self.incoming_rx.lock().await;
        rx.recv().await
    }

    /// 返回 CLI 通过 `system/init` 通报的 session id（如果已收到）。
    pub async fn cli_session_id(&self) -> Option<String> {
        self.cli_session_id.lock().await.clone()
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(sid): Path<String>,
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let got_path = format!("/ws/cli/{sid}");
    if got_path != state.expected_path {
        tracing::warn!(
            expected = %state.expected_path,
            got = %got_path,
            %peer,
            "ws_bridge: rejected connection (session id mismatch)"
        );
        return Response::builder()
            .status(403)
            .body(axum::body::Body::from("session id mismatch"))
            .unwrap();
    }
    tracing::info!(%peer, path = %got_path, "ws_bridge: CLI upgrading to WS");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // 通知 bind() 的调用方：连接建立。
    if let Some(tx) = state.connected.lock().await.take() {
        let _ = tx.send(());
    }

    // take outgoing_rx——第一个连上的 CLI 独占这条 chan。
    let outgoing_rx = state.outgoing_rx.lock().await.take();
    let write_task = match outgoing_rx {
        Some(mut rx) => tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let line = match serde_json::to_string(&msg) {
                    Ok(s) => s + "\n",
                    Err(e) => {
                        tracing::error!(error = %e, "ws_bridge: serialize outgoing failed");
                        continue;
                    }
                };
                if let Err(e) = sender.send(Message::Text(line.into())).await {
                    tracing::warn!(error = %e, "ws_bridge: send to CLI failed, writer exiting");
                    return rx;
                }
            }
            rx
        }),
        None => {
            tracing::warn!("ws_bridge: outgoing_rx already taken; this WS connection cannot write");
            // 起一个空 task 保持类型对齐；它立刻结束。
            tokio::spawn(async {
                let (_dummy_tx, dummy_rx) = mpsc::unbounded_channel::<Value>();
                dummy_rx
            })
        }
    };

    // 读循环：WS frame → NDJSON split → parse → incoming_tx。
    let incoming_tx = state.incoming_tx.clone();
    let session_slot = state.cli_session_id.clone();
    while let Some(frame) = receiver.next().await {
        match frame {
            Ok(Message::Text(text)) => {
                // NDJSON：一帧里可能有多条 JSON（anya 做了 buffer 但 axum 通常
                // 把每条 send 当成独立 frame——这里同时支持两种姿势）。
                let trimmed = text.trim_end_matches('\n');
                for line in trimmed.split('\n') {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(line) {
                        Ok(v) => {
                            // 本地 peek：抓 system/init 的 session_id
                            if v.get("type").and_then(Value::as_str) == Some("system")
                                && v.get("subtype").and_then(Value::as_str) == Some("init")
                                && let Some(sid) = v.get("session_id").and_then(Value::as_str)
                            {
                                *session_slot.lock().await = Some(sid.to_string());
                            }
                            if incoming_tx.send(v).is_err() {
                                tracing::debug!("ws_bridge: incoming_rx dropped; bailing reader");
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                line_preview = %line.chars().take(200).collect::<String>(),
                                "ws_bridge: JSON parse failed; line skipped"
                            );
                        }
                    }
                }
            }
            Ok(Message::Binary(_)) => {
                tracing::debug!("ws_bridge: unexpected binary frame, skipped");
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => { /* axum 自动处理 */ }
            Ok(Message::Close(_)) => {
                tracing::info!("ws_bridge: CLI closed WS");
                break;
            }
            Err(e) => {
                tracing::warn!(error = %e, "ws_bridge: WS read error; bailing");
                break;
            }
        }
    }

    // CLI 断开：让 write task 收尾（发送端会因 rx drop 退出）。
    drop(write_task);
    tracing::info!("ws_bridge: socket handler exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TkMsg;

    /// 端到端：起 WsChannel → 客户端伪装 CLI 连上 → 互发一跳。
    /// 用 tokio-tungstenite 作为"假 CLI"。
    #[tokio::test]
    async fn roundtrip_one_message_each_way() {
        let sid = "test-sid-001";
        let channel = WsChannel::bind(sid).await.expect("bind");
        let url = channel.url();

        // 客户端（假 CLI）连上来
        let (mut ws, _resp) = connect_async(&url).await.expect("connect");

        channel
            .wait_connect(Duration::from_secs(2))
            .await
            .expect("connect");

        // 服务端 → 客户端
        channel
            .send(serde_json::json!({"type":"user", "content":"hi"}))
            .expect("send");
        let received = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("recv timeout")
            .expect("some")
            .expect("ok");
        match received {
            TkMsg::Text(s) => {
                let v: Value = serde_json::from_str(s.trim_end_matches('\n')).expect("json");
                assert_eq!(v["type"], "user");
                assert_eq!(v["content"], "hi");
            }
            other => panic!("expected text, got {other:?}"),
        }

        // 客户端 → 服务端
        ws.send(TkMsg::Text(
            serde_json::to_string(&serde_json::json!({
                "type":"system", "subtype":"init",
                "session_id":"cli-sid-xyz", "model":"haiku", "cwd":"/tmp"
            }))
            .unwrap()
            .into(),
        ))
        .await
        .expect("send");

        let msg = tokio::time::timeout(Duration::from_secs(2), channel.recv())
            .await
            .expect("timeout")
            .expect("some");
        assert_eq!(msg["type"], "system");
        assert_eq!(msg["subtype"], "init");

        // system/init 应已被 peek 到
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            channel.cli_session_id().await,
            Some("cli-sid-xyz".to_string())
        );
    }

    #[tokio::test]
    async fn connect_timeout_when_no_client() {
        let channel = WsChannel::bind("nobody").await.expect("bind");
        let err = channel
            .wait_connect(Duration::from_millis(150))
            .await
            .expect_err("should timeout");
        match err {
            WsError::ConnectTimeout(_) => {}
            other => panic!("expected ConnectTimeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_wrong_session_id() {
        let channel = WsChannel::bind("real-sid").await.expect("bind");
        let wrong_url = format!("ws://127.0.0.1:{}/ws/cli/WRONG", channel.port);
        // tungstenite 会在 upgrade 403 时报错
        let res = connect_async(&wrong_url).await;
        assert!(res.is_err(), "expected handshake error, got {res:?}");
    }
}
