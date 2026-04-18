//! A2A crate 的统一错误类型。
//!
//! 为什么自己定义而不是直接复用 `fuxi_core::CoreError`——本 crate 涉及
//! HTTP/JSON-RPC 语义，需要区分 "远端协议错" 与 "本地执行错"，
//! 而且 CoreError 不感知 reqwest/axum。对外仍然通过 `#[from]` 收纳。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("http client: {0}")]
    Http(#[from] reqwest::Error),

    #[error("axum: {0}")]
    Axum(#[from] axum::Error),

    #[error("core: {0}")]
    Core(#[from] fuxi_core::CoreError),

    /// 对端返回了一个 JSON-RPC `error` 对象——这是 "合法的远端失败"，
    /// 需要把协议层的 code/message 一路带出去给上层决策。
    #[error("jsonrpc error {code}: {message}")]
    JsonRpc { code: i32, message: String },

    /// 本地业务拒绝：任务不存在、状态机拒绝、非法参数等。
    #[error("a2a: {0}")]
    A2A(String),

    #[error("invalid url: {0}")]
    InvalidUrl(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// JSON-RPC 规范规定的 internal error code——业务层失败时默认用这个。
    pub const CODE_INTERNAL: i32 = -32603;
    pub const CODE_INVALID_PARAMS: i32 = -32602;
    pub const CODE_METHOD_NOT_FOUND: i32 = -32601;
    pub const CODE_PARSE: i32 = -32700;

    pub(crate) fn jsonrpc_code(&self) -> i32 {
        match self {
            Error::JsonRpc { code, .. } => *code,
            Error::Serde(_) => Self::CODE_PARSE,
            _ => Self::CODE_INTERNAL,
        }
    }
}
