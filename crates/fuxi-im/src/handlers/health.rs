//! `/healthz`——liveness probe。
//!
//! 不依赖 state；nginx 上游 health check 直接打这条。

pub async fn healthz() -> &'static str {
    "ok"
}
