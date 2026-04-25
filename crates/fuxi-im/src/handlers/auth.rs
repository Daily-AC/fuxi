//! `/api/auth/pair` —— 设备一次性 PIN 配对（β 接管）。
//!
//! 流程见 Decision 14 D：TUI `/pair` 出 6 位 PIN → 手机 POST 回来 → 服务端
//! HMAC 签 device token 写 cookie，1 年 TTL。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PairBody {
    pub pin: String,
    pub device_name: String,
}

pub async fn pair(
    State(_state): State<AppState>,
    Json(_body): Json<PairBody>,
) -> Result<&'static str> {
    Err(Error::NotImplemented("POST /api/auth/pair"))
}
