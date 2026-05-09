//! WS 端到端集成测——起真实 axum + 自连 WS client + MockEngine。
//!
//! 思路同 `crates/fuxi-im/tests/ws_stream.rs`：起 ephemeral 端口的 axum，
//! tokio-tungstenite 当 client，断帧反序列化回 `ServerMessage` / `ClientMessage`
//! 验契约。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use fuxi_wake_server::engine::WakeEngine;
use fuxi_wake_server::engine::mock::MockEngine;
use fuxi_wake_server::protocol::{ClientMessage, ServerMessage};
use fuxi_wake_server::{AppState, router};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;

const TEST_TOKEN: &str = "test-secret-token";

async fn spawn_server(token: String) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let state = AppState::new(token, || Box::new(MockEngine::new()) as Box<dyn WakeEngine>);
    let app = router(Arc::new(state));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let h = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    (addr, h)
}

fn ws_url(addr: SocketAddr) -> String {
    format!("ws://{addr}/api/wake")
}

async fn connect_with_token(
    addr: SocketAddr,
    bearer: Option<&str>,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    let url = ws_url(addr);
    let mut req = url.as_str().into_client_request().expect("into req");
    if let Some(t) = bearer {
        req.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {t}")).expect("hv"),
        );
    }
    connect_async(req).await
}

#[tokio::test]
async fn missing_bearer_returns_401() {
    let (addr, _h) = spawn_server(TEST_TOKEN.into()).await;
    let err = connect_with_token(addr, None).await.unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("401"), "应 401，实际错误: {s}");
}

#[tokio::test]
async fn bad_bearer_returns_401() {
    let (addr, _h) = spawn_server(TEST_TOKEN.into()).await;
    let err = connect_with_token(addr, Some("wrong")).await.unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("401"), "应 401，实际错误: {s}");
}

#[tokio::test]
async fn good_bearer_upgrades_and_emits_ready_after_hello() {
    let (addr, _h) = spawn_server(TEST_TOKEN.into()).await;
    let (mut ws, resp) = connect_with_token(addr, Some(TEST_TOKEN))
        .await
        .expect("connect");
    assert_eq!(resp.status().as_u16(), 101);

    let hello = ClientMessage::Hello {
        client: "jarvis-mac".into(),
        version: "0.1.0".into(),
    };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .expect("send hello");

    let got = next_text(&mut ws).await;
    let parsed: ServerMessage = serde_json::from_str(&got).expect("parse");
    match parsed {
        ServerMessage::Ready { keywords } => {
            assert_eq!(keywords, vec!["玄女".to_string()]);
        }
        other => panic!("expect ready, got {other:?}"),
    }
}

#[tokio::test]
async fn mock_engine_emits_wake_after_pcm_with_advanced_clock() {
    // MockEngine 真 30s 间隔——这里直接拉一个内部 engine，把锚点搬到 31s 前后再 feed
    // 验唤醒事件能被 server 推到 client。完整 WS 路径不强求"真等 30s"，与上 IM
    // 端到端测一致：单元层验"feed 命中→Wake 转发"，时间用同步技巧推。
    //
    // 做法：用一份"立刻命中"的 InstantEngine 替代 MockEngine 当 factory。
    use anyhow::Result;
    use async_trait::async_trait;

    struct InstantEngine;
    #[async_trait]
    impl WakeEngine for InstantEngine {
        async fn init(&self, _kw: &[String]) -> Result<()> {
            Ok(())
        }
        async fn feed(&self, _pcm: &[u8]) -> Result<Option<(String, f32)>> {
            Ok(Some(("玄女".into(), 0.95)))
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
    }

    let state = AppState::new(TEST_TOKEN.into(), || {
        Box::new(InstantEngine) as Box<dyn WakeEngine>
    });
    let app = router(Arc::new(state));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let _h = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });

    let (mut ws, _) = connect_with_token(addr, Some(TEST_TOKEN))
        .await
        .expect("connect");

    let hello = ClientMessage::Hello {
        client: "jarvis-mac".into(),
        version: "0.1.0".into(),
    };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .expect("send hello");

    // ready
    let _ready = next_text(&mut ws).await;

    // 灌一帧二进制 PCM——InstantEngine 立刻命中。
    ws.send(Message::Binary(vec![0u8; 1280].into()))
        .await
        .expect("send pcm");

    // 接下来应收到 wake；中间可能夹 ping，跳过非 wake/error 帧。
    let evt = next_server_message_matching(&mut ws, |m| {
        matches!(m, ServerMessage::Wake { .. } | ServerMessage::Error { .. })
    })
    .await;
    match evt {
        ServerMessage::Wake {
            keyword,
            score,
            at: _,
        } => {
            assert_eq!(keyword, "玄女");
            assert!(score > 0.0);
        }
        other => panic!("expect wake, got {other:?}"),
    }
}

#[tokio::test]
async fn server_sends_ping_on_interval() {
    // 5s ping 间隔——本测验"hello 后 ≤ 7s 内能拿到至少一条 ping"。
    let (addr, _h) = spawn_server(TEST_TOKEN.into()).await;
    let (mut ws, _) = connect_with_token(addr, Some(TEST_TOKEN))
        .await
        .expect("connect");

    let hello = ClientMessage::Hello {
        client: "jarvis-mac".into(),
        version: "0.1.0".into(),
    };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .expect("send hello");
    // 给 ready 用足够预算（≥ 5s + ready 即时下发）。
    let _ready = next_text_within(&mut ws, Duration::from_secs(2)).await;

    let evt = next_server_message_matching_within(
        &mut ws,
        |m| matches!(m, ServerMessage::Ping { .. }),
        Duration::from_secs(7),
    )
    .await;

    assert!(matches!(evt, ServerMessage::Ping { .. }));
}

#[tokio::test]
async fn bad_first_frame_emits_audio_format_invalid() {
    let (addr, _h) = spawn_server(TEST_TOKEN.into()).await;
    let (mut ws, _) = connect_with_token(addr, Some(TEST_TOKEN))
        .await
        .expect("connect");

    // 第一帧给 pong 而非 hello——server 应回 error("audio_format_invalid") 后断。
    let pong = ClientMessage::Pong {
        at: chrono::Utc::now(),
    };
    ws.send(Message::Text(serde_json::to_string(&pong).unwrap().into()))
        .await
        .expect("send pong");

    let got = next_text(&mut ws).await;
    let parsed: ServerMessage = serde_json::from_str(&got).expect("parse");
    match parsed {
        ServerMessage::Error { code, .. } => {
            assert_eq!(code, "audio_format_invalid");
        }
        other => panic!("expect error, got {other:?}"),
    }
}

#[tokio::test]
async fn health_endpoint_returns_ok_json() {
    let (addr, _h) = spawn_server(TEST_TOKEN.into()).await;
    let url = format!("http://{addr}/health");
    let body = reqwest_get_text(&url).await;
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["status"], "ok");
    assert!(v["sdk"].is_string());
    assert_eq!(v["awake_count"].as_u64(), Some(0));
}

// ---- helpers ----

async fn next_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    next_text_within(ws, Duration::from_secs(3)).await
}

async fn next_text_within(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    budget: Duration,
) -> String {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("等帧超时");
        }
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("等帧超时")
            .expect("流关")
            .expect("ws err");
        match msg {
            Message::Text(t) => return t.to_string(),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) | Message::Binary(_) => {
                continue;
            }
            Message::Close(c) => panic!("server 主动 close: {c:?}"),
        }
    }
}

async fn next_server_message_matching<F>(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    pred: F,
) -> ServerMessage
where
    F: FnMut(&ServerMessage) -> bool,
{
    next_server_message_matching_within(ws, pred, Duration::from_secs(3)).await
}

async fn next_server_message_matching_within<F>(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut pred: F,
    budget: Duration,
) -> ServerMessage
where
    F: FnMut(&ServerMessage) -> bool,
{
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("等待匹配 server 消息超时");
        }
        let t = next_text_within(ws, remaining).await;
        let parsed: ServerMessage = serde_json::from_str(&t).expect("parse server msg");
        if pred(&parsed) {
            return parsed;
        }
    }
}

async fn reqwest_get_text(url: &str) -> String {
    // 不引 reqwest——自己用 hyper/tcp 太啰嗦，临时用 std + curl 不方便。
    // 用 tokio TcpStream + 手撕 HTTP/1.1 GET——只为 /health 一个端点。
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let parsed = url::Url::parse(url).expect("url");
    let host = parsed.host_str().expect("host").to_string();
    let port = parsed.port().expect("port");
    let mut s = tokio::net::TcpStream::connect(format!("{host}:{port}"))
        .await
        .expect("connect");
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        parsed.path(),
        host,
        port
    );
    s.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.expect("read");
    let raw = String::from_utf8_lossy(&buf).to_string();
    let body_start = raw.find("\r\n\r\n").expect("body start") + 4;
    raw[body_start..].to_string()
}
