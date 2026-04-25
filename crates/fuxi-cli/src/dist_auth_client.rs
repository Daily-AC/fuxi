//! worker → controller HMAC 签名 wrapper（path 3 β）。
//!
//! 所有 worker 端出站 HTTP 必须走 `signed_post` / `signed_get`，把请求的
//! method / path / timestamp / nonce / body 喂给 α 的 `sign_request` 算
//! HMAC-SHA256，挂 3 个 `x-fuxi-*` header；controller 端 `hmac_layer` 验签。
//!
//! ## 与 α 模块的边界
//!
//! 算法 + canonical string 顺序 + header 名 + secret 解析全在 `crate::dist_auth`，
//! 本模块只是 reqwest builder 上的薄封装：
//! - 共用 `HmacSecret` 让 worker 和 controller 解析 secret 字符串方式不漂移
//! - 共用 `sign_request` 让两端 canonical bytes 字节级一致
//! - 共用 `X_FUXI_*` header 名常量（α 是 lower-case；axum HeaderName 强制
//!   小写存储；reqwest 大小写不敏感发送，但用同一常量避免文档/字面值漂移）
//!
//! ## body 字节守恒
//!
//! `signed_post` 内部 `serde_json::to_vec` 一次，**同一份 bytes 既参与签名
//! 又作为请求体**——避免 reqwest `.json()` serializer 与手算的字节序列不
//! 一致导致签的字节 ≠ 发的字节（见 controller middleware：path/body 必须
//! byte-for-byte 一致才验得过）。

#![allow(dead_code)]

use crate::dist_auth::{
    HmacSecret, X_FUXI_NONCE, X_FUXI_SIGNATURE, X_FUXI_TIMESTAMP, fresh_nonce, now_unix_ms,
    sign_request,
};
use anyhow::{Context, Result};
use reqwest::{Client, Method, Response};
use serde::Serialize;
use std::sync::Arc;

/// 共享 secret 包装——worker spawn 的 task 间复制成本 0。
pub type SharedSecret = Arc<HmacSecret>;

/// 抽 path（不含 query / fragment）。canonical string 的 path 段必须与
/// controller 端 `req.uri().path()` byte-for-byte 一致；query 不参与签名
/// 否则 reqwest 编码顺序会让两端算出不同 string（α middleware 的注释
/// 也明确说"query 仅做 routing hint，不在签名保护内"）。
fn extract_path(url: &str) -> &str {
    let scheme_end = url.find("://").map(|i| i + 3).unwrap_or(0);
    let after_scheme = &url[scheme_end..];
    let path_start = after_scheme
        .find('/')
        .map(|i| scheme_end + i)
        .unwrap_or(url.len());
    let tail = &url[path_start..];
    let end = tail.find(['?', '#']).unwrap_or(tail.len());
    &tail[..end]
}

/// 给 builder 挂 3 个签名 header；调用方负责 send。
fn attach_signature(
    builder: reqwest::RequestBuilder,
    secret: &HmacSecret,
    method: &str,
    url: &str,
    body: &[u8],
) -> reqwest::RequestBuilder {
    let ts = now_unix_ms();
    let nonce = fresh_nonce();
    let path = extract_path(url);
    let sig = sign_request(secret, method, path, ts, &nonce, body);
    builder
        .header(X_FUXI_TIMESTAMP, ts.to_string())
        .header(X_FUXI_NONCE, nonce)
        .header(X_FUXI_SIGNATURE, sig)
}

/// 签名后 POST。body 是任意 `Serialize` 类型——内部 `serde_json::to_vec`
/// 一次，同一份 bytes 既签又发。
pub async fn signed_post<T: Serialize + ?Sized>(
    client: &Client,
    secret: &HmacSecret,
    url: &str,
    body: &T,
) -> Result<Response> {
    let body_bytes = serde_json::to_vec(body).context("serialize signed_post body")?;
    let builder = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body_bytes.clone());
    let signed = attach_signature(builder, secret, Method::POST.as_str(), url, &body_bytes);
    signed.send().await.context("signed_post send")
}

/// 签名后 GET（无 body）。`query` 形如 `[("token","x"),("node_id","y")]`，
/// 走 reqwest `.query()` 与原 worker pull 路径一致；签名段 body 取空。
/// 注意 query 不在签名保护内（path-only 签名）。
pub async fn signed_get(
    client: &Client,
    secret: &HmacSecret,
    url: &str,
    query: &[(&str, &str)],
) -> Result<Response> {
    let builder = client.get(url).query(query);
    let signed = attach_signature(builder, secret, Method::GET.as_str(), url, &[]);
    signed.send().await.context("signed_get send")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist_auth::{HmacGate, hmac_layer};
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::middleware::from_fn_with_state;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// 起 echo 服务，捕获 3 个签名 header + body，回 200。**不挂 hmac_layer**——
    /// β unit 测只验 wrapper 自己挂的 header / 字节序列正确；真 middleware
    /// 端到端验签 by γ 覆盖。
    #[derive(Clone, Default)]
    struct CapturedHeaders {
        ts: Arc<Mutex<Option<String>>>,
        nonce: Arc<Mutex<Option<String>>>,
        sig: Arc<Mutex<Option<String>>>,
        body: Arc<Mutex<Option<Vec<u8>>>>,
    }

    async fn capture_post(
        State(cap): State<CapturedHeaders>,
        headers: HeaderMap,
        body: axum::body::Bytes,
    ) -> impl IntoResponse {
        *cap.ts.lock().unwrap() = headers
            .get(X_FUXI_TIMESTAMP)
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        *cap.nonce.lock().unwrap() = headers
            .get(X_FUXI_NONCE)
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        *cap.sig.lock().unwrap() = headers
            .get(X_FUXI_SIGNATURE)
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        *cap.body.lock().unwrap() = Some(body.to_vec());
        Json(json!({"ok": true}))
    }

    async fn capture_get(
        State(cap): State<CapturedHeaders>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        *cap.ts.lock().unwrap() = headers
            .get(X_FUXI_TIMESTAMP)
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        *cap.nonce.lock().unwrap() = headers
            .get(X_FUXI_NONCE)
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        *cap.sig.lock().unwrap() = headers
            .get(X_FUXI_SIGNATURE)
            .and_then(|h| h.to_str().ok())
            .map(String::from);
        Json(json!({"ok": true}))
    }

    async fn spawn_echo() -> (String, CapturedHeaders, tokio::task::JoinHandle<()>) {
        let cap = CapturedHeaders::default();
        let app = Router::new()
            .route("/dist/echo", post(capture_post).get(capture_get))
            .with_state(cap.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), cap, handle)
    }

    /// signed_post 必须挂齐 3 个 header，且 sig 与 α 的 sign_request 算出一致。
    #[tokio::test]
    async fn signed_post_attaches_three_headers_and_signature_matches() {
        let (base, cap, srv) = spawn_echo().await;
        let secret = HmacSecret::new("test-secret".into());
        let client = Client::new();
        let body = json!({"hello": "world"});
        let resp = signed_post(&client, &secret, &format!("{base}/dist/echo"), &body)
            .await
            .expect("signed_post send");
        assert_eq!(resp.status(), 200);

        let ts = cap.ts.lock().unwrap().clone().expect("missing ts header");
        let nonce = cap
            .nonce
            .lock()
            .unwrap()
            .clone()
            .expect("missing nonce header");
        let sig = cap.sig.lock().unwrap().clone().expect("missing sig header");
        let captured_body = cap.body.lock().unwrap().clone().expect("missing body");

        let ts_u64: u64 = ts.parse().expect("ts should parse as u64");
        // server 收到的 body 必须等于 wrapper 传的 body（同一份 bytes 签 + 发）
        assert_eq!(captured_body, serde_json::to_vec(&body).unwrap());

        // 用 α sign_request 重算 sig——必须 byte-for-byte 一致
        let expected = sign_request(
            &secret,
            "POST",
            "/dist/echo",
            ts_u64,
            &nonce,
            &captured_body,
        );
        assert_eq!(sig, expected, "wrapper 算的 sig 必须与 α sign_request 一致");
        srv.abort();
    }

    /// signed_get 无 body 时签的是空字节序列。
    #[tokio::test]
    async fn signed_get_with_no_body_signs_empty_string() {
        let (base, cap, srv) = spawn_echo().await;
        let secret = HmacSecret::new("test-secret".into());
        let client = Client::new();
        let resp = signed_get(
            &client,
            &secret,
            &format!("{base}/dist/echo"),
            &[("token", "tok"), ("node_id", "n1")],
        )
        .await
        .expect("signed_get send");
        assert_eq!(resp.status(), 200);

        let ts = cap.ts.lock().unwrap().clone().expect("ts");
        let nonce = cap.nonce.lock().unwrap().clone().expect("nonce");
        let sig = cap.sig.lock().unwrap().clone().expect("sig");
        let ts_u64: u64 = ts.parse().unwrap();
        let expected = sign_request(&secret, "GET", "/dist/echo", ts_u64, &nonce, &[]);
        assert_eq!(sig, expected);
        srv.abort();
    }

    /// 同一 url 连签两次 nonce 必不同（fresh_nonce 16 bytes hex）。
    #[tokio::test]
    async fn signed_post_uses_fresh_nonce_each_call() {
        let (base, cap, srv) = spawn_echo().await;
        let secret = HmacSecret::new("x".into());
        let client = Client::new();
        let body = json!({"k":"v"});

        signed_post(&client, &secret, &format!("{base}/dist/echo"), &body)
            .await
            .unwrap();
        let n1 = cap.nonce.lock().unwrap().clone().unwrap();

        signed_post(&client, &secret, &format!("{base}/dist/echo"), &body)
            .await
            .unwrap();
        let n2 = cap.nonce.lock().unwrap().clone().unwrap();

        assert_ne!(n1, n2, "每次调用应生成新 nonce 防 replay");
        srv.abort();
    }

    /// extract_path 去 query / fragment——必须与 axum `req.uri().path()` 一致。
    #[test]
    fn extract_path_strips_query_and_fragment() {
        assert_eq!(extract_path("http://h:1/dist/pull?token=x"), "/dist/pull");
        assert_eq!(extract_path("https://h/dist/pull"), "/dist/pull");
        assert_eq!(extract_path("http://h/a/b#frag"), "/a/b");
        assert_eq!(extract_path("http://h/"), "/");
    }

    /// **真实 middleware 验签**——把 α 的 hmac_layer 挂上，POST 必须 200。
    /// 这是 β 自己最强的端到端：wrapper → α middleware → handler 全链路通。
    /// γ 还会跑 replay/skew/tamper 等负面 case；这个只验"正路是通的"。
    #[tokio::test]
    async fn signed_post_passes_real_middleware_verify() {
        async fn ok(_body: axum::body::Bytes) -> impl IntoResponse {
            Json(json!({"ok": true}))
        }
        let secret = HmacSecret::new("real-middleware-test".into());
        let gate = HmacGate::new(secret.clone());
        let app = Router::new()
            .route("/dist/echo", post(ok))
            .layer(from_fn_with_state(gate, hmac_layer));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = Client::new();
        let body = json!({"hello": "from beta wrapper"});
        let resp = signed_post(&client, &secret, &format!("http://{addr}/dist/echo"), &body)
            .await
            .expect("signed_post");
        assert_eq!(resp.status(), 200, "α middleware 必须放行 wrapper 签的请求");
        srv.abort();
    }

    /// 真 middleware：signed_get + path-only 签名。
    #[tokio::test]
    async fn signed_get_passes_real_middleware_verify() {
        async fn ok() -> impl IntoResponse {
            Json(json!({"ok": true}))
        }
        let secret = HmacSecret::new("real-middleware-get".into());
        let gate = HmacGate::new(secret.clone());
        let app = Router::new()
            .route("/dist/pull", get(ok))
            .layer(from_fn_with_state(gate, hmac_layer));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = Client::new();
        let resp = signed_get(
            &client,
            &secret,
            &format!("http://{addr}/dist/pull"),
            &[("token", "x"), ("node_id", "n1")],
        )
        .await
        .expect("signed_get");
        assert_eq!(resp.status(), 200);
        srv.abort();
    }
}
