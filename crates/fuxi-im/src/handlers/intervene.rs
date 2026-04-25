//! `/api/intervene` + `/api/dispatch` —— 给玄女说话 / 强制开新 root task。
//!
//! 入站协议先定下来（`{ "text": "..." }`），handler 内部调 `Fuxi::intervene` /
//! `Fuxi::dispatch` 由后续 owner 实装。骨架阶段返 501。
//!
//! 决策 04：intervene idle 自动 degrade —— Fuxi::intervene 已处理，IM 层不要自己再写。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct InterveneBody {
    pub text: String,
    /// 是否打断当前 turn（CLAUDE.md 决策 04 中 `interrupt_first` 语义）。
    #[serde(default)]
    pub interrupt: bool,
}

pub async fn intervene(
    State(_state): State<AppState>,
    Json(_body): Json<InterveneBody>,
) -> Result<&'static str> {
    Err(Error::NotImplemented("POST /api/intervene"))
}
