//! 路由装配——按 `docs/decisions/14-im-mobile-frontend.md` C 节路由表挂齐。
//!
//! 设计取舍：现在 stub 也挂上路径 + 501，**不留 404 给未实装端点**。
//! 理由——前端开发期间能区分"端点不存在（路由错）" vs "端点未实装（owner 还没接）"。
//! 实装到位的 owner（β/γ/δ/ε）改对应 handler 即可，不动这份装配。
//!
//! 静态 PWA 资源（Decision 14 表中的 `GET /` `include_dir!`）由 ε 接管时
//! 在 `lib::router` 里 `.fallback_service` 挂——本文件只管 `/api` + `/healthz`。
//!
//! ## 鉴权 layer
//!
//! `cookie_auth_layer` 必须挂在 router 上才能拦请求——单独写好但没挂等于裸奔。
//! 历史 bug（2026-04-26 用户实测撞上）：`build()` 之前只 `.layer(TraceLayer)`，
//! 鉴权 layer 写好了但**从未集成**，所有 `/api/*` 都能无 cookie 直接访问。
//! 修复 = 这里挂上 + `tests/router_auth_integration.rs` 走 Router 真请求测试做门禁。
//! 隔离单测（直接调 layer fn）抓不到此类"忘记挂上"的 bug。

use crate::handlers;
use crate::middleware::{AuthGate, cookie_auth_layer};
use crate::state::AppState;
use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

/// 装配 IM 服务的全部路由。
///
/// 后续 owner 加新端点：在对应 `handlers/*.rs` 里实装真逻辑，本文件不动。
pub fn build(state: AppState) -> Router {
    let auth_gate = AuthGate::new(state.im_auth.secret.clone());
    Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/api/tasks", get(handlers::tasks::list_tasks))
        .route("/api/tasks/{id}/events", get(handlers::tasks::task_events))
        .route(
            "/api/tasks/{id}/stream",
            get(handlers::conv::task_stream_ws),
        )
        .route("/api/conv", get(handlers::conv::conv_ws))
        // β · #17 IM 层聊天记录拉取（首屏预加载 + 翻页）
        .route("/api/conv/messages", get(handlers::conv::messages))
        .route("/api/intervene", post(handlers::intervene::intervene))
        .route("/api/dispatch", post(handlers::dispatch::dispatch))
        // β · #17 文件上传 / 下载
        .route("/api/upload", post(handlers::upload::upload))
        .route("/api/uploads/{id}", get(handlers::upload::get_upload))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/pair", post(handlers::auth::pair))
        .route("/api/push/subscribe", post(handlers::push::subscribe))
        .route("/api/push/silence", post(handlers::push::silence))
        .route("/api/push/vapid-pub", get(handlers::push::vapid_public_key))
        // 鉴权 layer 在最外侧——所有 /api/* 都过它；is_exempt 内部豁免
        // /api/auth/login + /api/auth/pair + /healthz（这三条本身就是签发 cookie /
        // upstream check 的入口，自然不能要求带已有的 cookie）。
        // 非 /api/* 路径（PWA 静态资源 /、ε 的 fallback_service）也由 layer
        // is_exempt 之外的 "!path.starts_with('/api/')" 分支放行。
        .layer(from_fn_with_state(auth_gate, cookie_auth_layer))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
