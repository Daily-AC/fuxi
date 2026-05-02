//! `fuxi-im` 本地错误类型。
//!
//! 设计取舍参考 `fuxi-firehose/src/error.rs`：IM 层会同时碰到
//! "拿不到 Fuxi 句柄"（编排错）/ "请求体不合法"（用户错）/ "下游 events 错"
//! 三种来源，单独枚举让 handler 用 `?` 一次性收敛，并通过 `IntoResponse`
//! 统一翻译为 HTTP。
//!
//! 为什么不直接用 `anyhow::Error`：handler 里要按错误类型决定 4xx/5xx，
//! 字符串错误丢失类型信息。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::io;

/// 本 crate 专属错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O——bind / accept 等。
    #[error("io: {0}")]
    Io(#[from] io::Error),

    /// 请求体或查询参数不合法。
    #[error("bad request: {0}")]
    BadRequest(String),

    /// 鉴权失败——未配对或 token 失效。骨架阶段 stub 用，实装由 β 落地。
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// 资源不存在（task id / agent id 等）。
    #[error("not found: {0}")]
    NotFound(String),

    /// 资源已存在——比如 `POST /api/projects` 撞 id 唯一约束。走 409。
    #[error("conflict: {0}")]
    Conflict(String),

    /// 暂时不可用——玄女未就绪、密码未设等"现在不能服务，但路径正确"。
    /// 走 503，PWA 应在 N 秒后重试。
    #[error("service unavailable: {0}")]
    Unavailable(String),

    /// 未实装——给 β/γ/δ 的 stub handler 用，避免 panic。
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    /// JSON 序列化/反序列化错误。
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),

    /// 下层事件总线错误。
    #[error("events: {0}")]
    Events(#[from] fuxi_events::Error),

    /// 编排层错误透传——`/api/intervene` `/api/dispatch` 调 Fuxi 时常见。
    /// `AgentNotFound` 在 IntoResponse 单独翻 503（玄女未起场景，PWA 应能区分）。
    #[error("orchestrator: {0}")]
    Orchestrator(#[from] fuxi_orchestrator::OrchestratorError),

    /// 兜底——内部错误，对客户端隐藏细节。
    #[error("internal: {0}")]
    Internal(String),
}

/// 本 crate 的 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// 错误类型对应的 HTTP 状态码。
    pub fn status(&self) -> StatusCode {
        match self {
            Error::BadRequest(_) | Error::Json(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            // 玄女不在 → 503（让 PWA 区分"路由错"和"暂时不可用，请重试"）；
            // 其余编排错走 500。
            Error::Unavailable(_)
            | Error::Orchestrator(fuxi_orchestrator::OrchestratorError::AgentNotFound(_)) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Error::Io(_) | Error::Events(_) | Error::Orchestrator(_) | Error::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    message: String,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();
        let kind = match &self {
            Error::Io(_) => "io",
            Error::BadRequest(_) => "bad_request",
            Error::Unauthorized(_) => "unauthorized",
            Error::NotFound(_) => "not_found",
            Error::Conflict(_) => "conflict",
            Error::Unavailable(_) => "unavailable",
            Error::NotImplemented(_) => "not_implemented",
            Error::Json(_) => "json",
            Error::Events(_) => "events",
            Error::Orchestrator(fuxi_orchestrator::OrchestratorError::AgentNotFound(_)) => {
                "xuannv_unavailable"
            }
            Error::Orchestrator(_) => "orchestrator",
            Error::Internal(_) => "internal",
        };
        let body = ErrorBody {
            error: kind,
            message: self.to_string(),
        };
        (status, Json(body)).into_response()
    }
}
