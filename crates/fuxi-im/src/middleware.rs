//! Cookie 鉴权 layer——所有 `/api/*` 路由要带合法 `fuxi_im_token` cookie；
//! `/api/auth/login`（主密码登入）/ `/api/auth/pair`（PIN 降级 fallback）/
//! `/healthz`（nginx upstream check）三条豁免——前两者本身就是签发 cookie
//! 的入口，自然不能要求带已有的 cookie。
//!
//! 设计取舍：
//! - **不区分 401 原因**：缺 cookie / 解码失败 / 签名错 / 过期 → 一律 401，
//!   trace 区分原因。手机端只需"未授权 → 跳 PIN 输入页"。
//! - **路径白名单写死在 layer 里**：route-level layer 管不到"哪些路径走哪个 middleware"
//!   (axum 0.8 现状)；最简单是 layer 看 path 自决。等 axum 出更细的 per-route
//!   middleware 再说。

#![allow(dead_code)]

use crate::auth::{HmacSecret, extract_token_from_cookie_header, verify_token};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use tracing::warn;

/// 从 `?a=b&token=xyz&c=d` 形式找 token 值。手解析省 url crate dep（lib 没装）。
/// token 值若 URL-encoded 不做 decode——HMAC token 是 base64url 不含特殊字符，
/// 客户端如错塞了 encoded 'token=%xx' 让它撞验签失败更安全。
fn extract_token_from_query(q: &str) -> Option<&str> {
    for pair in q.split('&') {
        if let Some(v) = pair.strip_prefix("token=") {
            return Some(v);
        }
    }
    None
}

/// 给 layer 用的状态——共享的 HMAC secret。
#[derive(Clone)]
pub struct AuthGate {
    pub secret: Arc<HmacSecret>,
}

impl AuthGate {
    pub fn new(secret: Arc<HmacSecret>) -> Self {
        Self { secret }
    }
}

/// 不需要鉴权的路径前缀——登入入口、配对入口本身、liveness probe。
/// `/api/dist/setup-worker` 是 worker onboarding 入口（本身用主密码鉴权而非
/// cookie，攻击面跟 `/api/auth/login` 同等）。
fn is_exempt(path: &str) -> bool {
    matches!(
        path,
        "/api/auth/login"
            | "/api/auth/pair"
            | "/api/dist/setup-worker"
            // bug #77 · /api/version 给 PWA 启动时 cache bust 用，未登入也能拉
            | "/api/version"
            | "/healthz"
    )
}

/// axum middleware 入口。
pub async fn cookie_auth_layer(
    State(gate): State<AuthGate>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();

    if is_exempt(path) {
        return next.run(request).await;
    }

    // /api/* 才走鉴权——其余（GET / 静态资源）放行让 ε 的 PWA 静态资源处理。
    // PWA shell 必须公开可拉，否则手机连"输 PIN 页"都打不开。
    if !path.starts_with("/api/") {
        return next.run(request).await;
    }

    // 鉴权解析三通道（优先级 Bearer > Query > Cookie）：
    //   1. Authorization Bearer——非浏览器客户端（curl / 第三方）走的主路
    //   2. URL ?token=<...>——浏览器 WebSocket 走的；Web WebSocket API 不能 set
    //      自定义 header，token 只能塞 URL；桌宠 Tauri webview WS 必须靠它，
    //      fetch 也用它作 fallback
    //   3. Cookie `fuxi_im_token`——PWA / 药丸 v0.2 主路；HttpOnly 防 XSS
    // 三条都用同一颗 HMAC secret 验签，安全等价；client 选哪条按自己环境最方便。
    let token: Option<String> = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")))
        .map(|s| s.trim().to_string())
        .or_else(|| {
            // 解析 query string ?token=...
            request
                .uri()
                .query()
                .and_then(extract_token_from_query)
                .map(str::to_string)
        })
        .or_else(|| {
            request
                .headers()
                .get(header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(extract_token_from_cookie_header)
                .map(str::to_string)
        });

    let Some(token) = token else {
        warn!(path, reason = "missing_token", "im auth reject");
        return unauthorized();
    };

    match verify_token(&gate.secret, &token) {
        Ok(_claims) => {
            // claims 暂不通过 extension 注入下游——handler 没人需要 device_id。
            // 等真有 handler 要审计是哪台设备来的再加 `request.extensions_mut().insert(claims)`。
            next.run(request).await
        }
        Err(e) => {
            warn!(path, error = %e, "im auth reject");
            unauthorized()
        }
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Body::from("unauthorized")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{COOKIE_NAME, fresh_claims, sign_token};
    use axum::Router;
    use axum::body::to_bytes;
    use axum::http::Request as HttpRequest;
    use axum::middleware::from_fn_with_state;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn fixture_secret() -> HmacSecret {
        HmacSecret::from_string("test-key".into())
    }

    fn make_app(gate: AuthGate) -> Router {
        Router::new()
            .route("/api/intervene", post(|| async { "ok" }))
            .route("/api/auth/pair", post(|| async { "pair-stub" }))
            .route("/healthz", get(|| async { "ok" }))
            .route("/", get(|| async { "index" }))
            .layer(from_fn_with_state(gate, cookie_auth_layer))
    }

    async fn do_req(app: Router, uri: &str, cookie: Option<&str>) -> (StatusCode, String) {
        let mut builder = HttpRequest::builder().method("POST").uri(uri);
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, c);
        }
        let req = builder.body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn rejects_protected_route_without_cookie() {
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let (status, body) = do_req(make_app(gate), "/api/intervene", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("unauthorized"));
    }

    #[tokio::test]
    async fn rejects_protected_route_with_expired_token() {
        let secret = fixture_secret();
        // 过期 1 秒
        let claims = crate::auth::TokenClaims {
            device_id: "x".into(),
            name: "x".into(),
            expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
        };
        let token = sign_token(&secret, &claims).unwrap();
        let gate = AuthGate::new(Arc::new(secret));
        let cookie = format!("{COOKIE_NAME}={token}");
        let (status, _) = do_req(make_app(gate), "/api/intervene", Some(&cookie)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_protected_route_with_valid_token() {
        let secret = fixture_secret();
        let claims = fresh_claims("dev-001".into(), "iPhone".into());
        let token = sign_token(&secret, &claims).unwrap();
        let gate = AuthGate::new(Arc::new(secret));
        let cookie = format!("{COOKIE_NAME}={token}");
        let app = make_app(gate);

        // 用 /api/intervene 验通过——POST 简单些。
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/api/intervene")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn pair_endpoint_is_exempt() {
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let (status, body) = do_req(make_app(gate), "/api/auth/pair", None).await;
        // 未带 cookie 也能进——配对的入口必然 unauth
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("pair-stub"));
    }

    #[tokio::test]
    async fn healthz_is_exempt() {
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let req = HttpRequest::builder()
            .method("GET")
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();
        let resp = make_app(gate).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_api_path_is_passthrough() {
        // 静态资源（/）不该被 layer 拦——PWA 入口必须公开
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let req = HttpRequest::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = make_app(gate).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn accepts_query_token_for_websocket_clients() {
        // Web WebSocket API 不能 set 自定义 header，token 必须走 URL ?token=
        // 桌宠 Tauri webview 的 FuxiClient.ts /api/conv WS 唯一通道
        let secret = fixture_secret();
        let claims = fresh_claims("dev-pet".into(), "jarvis-pet".into());
        let token = sign_token(&secret, &claims).unwrap();
        let gate = AuthGate::new(Arc::new(secret));
        let app = make_app(gate);

        let req = HttpRequest::builder()
            .method("POST")
            .uri(format!("/api/intervene?token={token}&from=cursor"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn query_token_with_invalid_signature_rejected() {
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let app = make_app(gate);
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/api/intervene?token=not.valid")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_bearer_token_for_non_browser_clients() {
        // 桌宠 Tauri webview / curl / 任何非浏览器客户端通过 Authorization Bearer
        // 鉴权——webview 不能 set Cookie header（forbidden name）。
        let secret = fixture_secret();
        let claims = fresh_claims("dev-pet".into(), "jarvis-pet".into());
        let token = sign_token(&secret, &claims).unwrap();
        let gate = AuthGate::new(Arc::new(secret));
        let app = make_app(gate);

        let req = HttpRequest::builder()
            .method("POST")
            .uri("/api/intervene")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn bearer_with_invalid_signature_rejected() {
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let app = make_app(gate);
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/api/intervene")
            .header(header::AUTHORIZATION, "Bearer not.a.valid.token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_cookie_with_unknown_name() {
        let gate = AuthGate::new(Arc::new(fixture_secret()));
        let cookie = "session=abc; other=def";
        let (status, _) = do_req(make_app(gate), "/api/intervene", Some(cookie)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
