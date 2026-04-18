//! `fuxi-firehose` · 实时 WebSocket + SSE + TUI 观察器。
//!
//! 职责：
//! - 把 `fuxi_events::EventBus` 通过 HTTP/WebSocket/SSE 暴露给外部订阅者（`hub`）；
//! - 为 TUI 和未来 Rust 消费者提供一条“连上来就拿到 `Stream<Event>`”的客户端（`client`）；
//! - 提供可复用的 ratatui Firehose 仪表盘控件（`tui`）。
//!
//! 对应公理：
//! - #3 真实时不轮询——WS/SSE 都是 push；REST `/events` 仅作历史检索，不长轮询。
//! - #5 SQLite 单一真相源——history 直接从 `EventStore.replay` 走。

pub mod client;
pub mod error;
pub mod hub;
pub mod tui;

pub use client::{EventStream, connect_sse, connect_ws};
pub use error::{Error, Result};
pub use hub::{
    DEFAULT_HISTORY_LIMIT, HistoryQuery, Hub, MAX_HISTORY_LIMIT, SubscribeQuery, WS_PING_INTERVAL,
    router,
};
pub use tui::{EventRow, FirehoseApp, MAX_BUFFER, RATE_WINDOW};
