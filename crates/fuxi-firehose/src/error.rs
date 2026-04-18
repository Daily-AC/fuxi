//! `fuxi-firehose` 的本地错误类型。
//!
//! 为什么不直接复用 `fuxi_events::Error`：观察器层会碰到网络/WebSocket/TUI 专属错误
//! （如握手失败、断流），单独一个枚举让上层精确辨别是数据源问题还是传输问题。

use std::io;

/// 本 crate 专属错误类型。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 底层 I/O 故障——主要是 TCP accept/connect。
    #[error("io: {0}")]
    Io(#[from] io::Error),

    /// axum/tower/hyper 层 HTTP 错误。
    #[error("http: {0}")]
    Http(String),

    /// WebSocket 握手或传输错误（客户端侧）。
    #[error("websocket: {0}")]
    WebSocket(String),

    /// SSE/HTTP 客户端错误（客户端侧）。
    #[error("reqwest: {0}")]
    Reqwest(#[from] reqwest::Error),

    /// URL 解析错误。
    #[error("url: {0}")]
    Url(#[from] url::ParseError),

    /// JSON 解析/序列化失败。
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),

    /// 下层事件总线错误。
    #[error("events: {0}")]
    Events(#[from] fuxi_events::Error),

    /// 非法 UUID 字符串（通常来自查询参数）。
    #[error("uuid: {0}")]
    Uuid(#[from] uuid::Error),

    /// 非法日期/时间字符串（查询参数）。
    #[error("chrono: {0}")]
    Chrono(#[from] chrono::ParseError),

    /// 其他。
    #[error("{0}")]
    Other(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(e.to_string())
    }
}

/// 本 crate 的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;
