//! `/api/push/subscribe` —— Web Push 订阅注册（δ 接管）。
//!
//! 自签 VAPID 见 Decision 14 E。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PushSubscription {
    pub endpoint: String,
    pub keys: PushKeys,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

pub async fn subscribe(
    State(_state): State<AppState>,
    Json(_sub): Json<PushSubscription>,
) -> Result<&'static str> {
    Err(Error::NotImplemented("POST /api/push/subscribe"))
}
