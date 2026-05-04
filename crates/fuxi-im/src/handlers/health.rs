//! `/healthz`——liveness probe。
//!
//! 不依赖 state；nginx 上游 health check 直接打这条。

use axum::Json;
use serde::Serialize;

pub async fn healthz() -> &'static str {
    "ok"
}

/// bug #77 · `/api/version`——build-time 注入的 git sha + 时间戳。
///
/// PWA 启动 + 60s 轮询拉这个，跟 localStorage 缓存比；不一致 → 强 reload
/// 跳过 SW 缓存，让用户立即看到新部署（绕过 iOS Safari SW 接管慢的问题）。
///
/// 字段：
/// - sha: git rev-parse --short HEAD（build.rs 注入；fail "unknown"）
/// - build_at: build 时刻 RFC3339
#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub sha: &'static str,
    pub build_at: &'static str,
}

pub async fn version() -> Json<VersionInfo> {
    Json(VersionInfo {
        sha: env!("FUXI_BUILD_SHA"),
        build_at: env!("FUXI_BUILD_TS"),
    })
}
