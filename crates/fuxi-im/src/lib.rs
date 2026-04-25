//! `fuxi-im` —— 伏羲 IM 后端 axum 服务。
//!
//! 给手机端 PWA 暴露任务/对话/intervene/web push 等端点；网络拓扑见
//! `docs/decisions/14-im-mobile-frontend.md`：
//!   `https://im.qmledmq.cn:8443` (nginx) → `127.0.0.1:9100` (本 crate)。
//!
//! 本 crate 是**协议骨架**：
//!   - `router(state)` 装配全部路由
//!   - `AppState { fuxi: Arc<Fuxi> }` 给 handler 共享句柄
//!   - 路由按 Decision 14 表全挂上；未实装 handler 返 501 而非 404
//!
//! β/γ/δ/ε 各 owner 在 `handlers/*.rs` 里把 stub 翻成真实装即可，
//! 不动 `router.rs` 装配。

mod handlers;
mod router;

pub mod auth;
pub mod db;
pub mod devices;
pub mod error;
pub mod middleware;
pub mod pair;
pub mod push;
pub mod state;

pub use error::{Error, Result};
pub use state::AppState;

use axum::Router;

/// 构造 IM 服务的 axum Router。调用方负责 `axum::serve(listener, router)`。
///
/// `state` 内带 `Arc<Fuxi>`——所有需要编排能力的 handler 通过 `State<AppState>`
/// 拿到。和 `fuxi-firehose::router` 同款契约。
pub fn router(state: AppState) -> Router {
    router::build(state)
}
