//! fuxi-wake-server——Linux 唤醒守护，给 Mac 贾维斯反向通道。
//!
//! 协议契约见 `apps/jarvis/WAKE_PROTOCOL.md`。这里实装 server 端：
//! - axum :9101 暴露 `/api/wake` (WS) + `/health`
//! - WakeEngine trait（mock + xfyun stub）
//! - Bearer token 鉴权 + 5s ping / 15s 入站超时 / mock 30s 唤醒间隔（≥ 1.5s 去重）

pub mod auth;
pub mod engine;
pub mod protocol;
pub mod server;

pub use server::{AppState, SdkStatus, router};
