//! `fuxi-events` · 伏羲的事件总线。
//!
//! - 进程内 `tokio::sync::broadcast` 实现 N 个订阅者的扇出；
//! - 每条事件都以 append-only 方式落到 SQLite（WAL）用于审计和重放；
//! - 发布方永不阻塞：落库走后台 writer 任务 + 有界 mpsc；若 writer 拥塞，
//!   追加一条 `Custom { label: "event_store_lagged", payload: { pending } }`
//!   哨兵事件，但**不丢失调用方原事件**；
//! - `replay(ReplayCursor)` 返回历史事件流，可选 `live_tail` 拼接实时广播。
//!
//! 公理对应：
//! - #3 真实时不轮询 → `subscribe()` 是推送；
//! - #5 SQLite 单一真相源 → `EventStore` 负责持久化。

pub mod bus;
pub mod store;

pub use bus::{EventBus, EventBusConfig};
pub use store::{EventStore, EventStream, ReplayCursor};

/// 本 crate 专属错误类型。包装 sqlx + core 错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQLite / sqlx 层错误。
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// 序列化/反序列化失败。
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),
    /// 透传 `fuxi-core` 错误以便上层统一处理。
    #[error("core: {0}")]
    Core(#[from] fuxi_core::CoreError),
    /// EventBus 已关闭（writer 任务不可用）。
    #[error("event bus writer 已关闭")]
    WriterClosed,
    /// 其他。
    #[error("{0}")]
    Other(String),
}

/// 本 crate 的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;
