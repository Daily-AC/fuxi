//! 路由装配——按 `docs/decisions/14-im-mobile-frontend.md` C 节路由表挂齐。
//!
//! 设计取舍：现在 stub 也挂上路径 + 501，**不留 404 给未实装端点**。
//! 理由——前端开发期间能区分"端点不存在（路由错）" vs "端点未实装（owner 还没接）"。
//! 实装到位的 owner（β/γ/δ/ε）改对应 handler 即可，不动这份装配。
//!
//! 静态 PWA 资源（Decision 14 表中的 `GET /` `include_dir!`）由 ε 接管时
//! 在 `lib::router` 里 `.fallback_service` 挂——本文件只管 `/api` + `/healthz`。

use crate::handlers;
use crate::state::AppState;
use axum::Router;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

/// 装配 IM 服务的全部路由。
///
/// 后续 owner 加新端点：在对应 `handlers/*.rs` 里实装真逻辑，本文件不动。
pub fn build(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(handlers::health::healthz))
        .route("/api/tasks", get(handlers::tasks::list_tasks))
        .route("/api/tasks/{id}/events", get(handlers::tasks::task_events))
        .route(
            "/api/tasks/{id}/stream",
            get(handlers::conv::task_stream_ws),
        )
        .route("/api/conv", get(handlers::conv::conv_ws))
        .route("/api/intervene", post(handlers::intervene::intervene))
        .route("/api/dispatch", post(handlers::dispatch::dispatch))
        .route("/api/auth/pair", post(handlers::auth::pair))
        .route("/api/push/subscribe", post(handlers::push::subscribe))
        .route("/api/push/silence", post(handlers::push::silence))
        .route("/api/push/vapid-pub", get(handlers::push::vapid_public_key))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
