//! 唤醒引擎抽象——上层 WS handler 不关心是讯飞还是 mock。

use anyhow::Result;
use async_trait::async_trait;

pub mod mock;
pub mod xfyun;

/// 唤醒命中——SDK 内部累计音频，单帧不命中返 None 是常态。
#[async_trait]
pub trait WakeEngine: Send + Sync {
    /// 启动 SDK session，注册关键词集。
    async fn init(&self, keywords: &[String]) -> Result<()>;

    /// 喂 16kHz mono s16le PCM 帧。命中 → `Some((keyword, score))`。
    async fn feed(&self, pcm: &[u8]) -> Result<Option<(String, f32)>>;

    /// 释放 SDK 资源——连接关闭时调一次。
    async fn close(&self) -> Result<()>;
}
