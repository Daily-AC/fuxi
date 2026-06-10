//! `/api/voice/tokens` —— PWA 语音链路的鉴权桥。
//!
//! PWA 登录态是 HttpOnly cookie，JS 读不到原始 token；而语音三件套各有口子：
//! asr（WS start 帧）/ tts（Bearer header）验同一颗 `im_hmac.key` 签的 HMAC
//! token，wake server 验独立预共享 `wake.token`。本端点站在 cookie 鉴权层
//! 后面，给已登录前端换出这两颗 token。
//!
//! im_token 现 mint 现发（30 天 TTL，比 cookie 的 1 年短——前端每次会话启动
//! 重新换，不落 localStorage 长期存）；wake_token 直读文件。

use axum::Json;
use axum::extract::State;
use chrono::{Duration, Utc};
use serde::Serialize;

use crate::auth::{TokenClaims, sign_token};
use crate::error::Result;
use crate::state::AppState;

/// 语音 token TTL。够覆盖一次长会话 + 前端缓存在内存；泄露面比 1 年 cookie 小。
const VOICE_TOKEN_TTL_DAYS: i64 = 30;

#[derive(Debug, Serialize)]
pub struct VoiceTokensResponse {
    /// intervene/asr/tts 共用的 HMAC token（`body.sig` 两段式）。
    pub im_token: String,
    /// wake server 预共享 token；未配置/文件缺失为 None——前端据此隐藏唤醒开关。
    pub wake_token: Option<String>,
}

pub async fn voice_tokens(State(state): State<AppState>) -> Result<Json<VoiceTokensResponse>> {
    let claims = TokenClaims {
        device_id: "pwa-voice".into(),
        name: "pwa-voice".into(),
        expires_at: Utc::now() + Duration::days(VOICE_TOKEN_TTL_DAYS),
    };
    let im_token = sign_token(&state.im_auth.secret, &claims)?;

    let wake_token = match &state.wake_token_path {
        Some(path) => match tokio::fs::read_to_string(path).await {
            Ok(s) => {
                let t = s.trim();
                (!t.is_empty()).then(|| t.to_string())
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "wake.token 读取失败，wake_token 降级 null");
                None
            }
        },
        None => None,
    };

    Ok(Json(VoiceTokensResponse {
        im_token,
        wake_token,
    }))
}
