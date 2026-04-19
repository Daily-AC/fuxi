//! fuxi-scheduler 错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] sqlx::Error),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("cron: {0}")]
    Cron(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("trigger disabled: {0}")]
    Disabled(String),

    #[error("fs watcher: {0}")]
    Watcher(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
