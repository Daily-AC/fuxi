//! 策府错误类型。

use fuxi_core::CoreError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("events: {0}")]
    Events(#[from] fuxi_events::Error),
    #[error("core: {0}")]
    Core(#[from] CoreError),
    /// 试图操作不存在的 id（fact / pattern）。
    #[error("memory row not found: {0}")]
    NotFound(String),
    /// 参数非法（如 confidence 越界）。
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
