//! `/api/dispatch` —— 强制开新 root task（独立于 intervene degrade 路径）。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct DispatchBody {
    pub title: String,
    pub text: String,
}

pub async fn dispatch(
    State(_state): State<AppState>,
    Json(_body): Json<DispatchBody>,
) -> Result<&'static str> {
    Err(Error::NotImplemented("POST /api/dispatch"))
}
