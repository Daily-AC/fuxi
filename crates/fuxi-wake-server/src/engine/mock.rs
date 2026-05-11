//! Mock 引擎——给 mac 端联调用，绕开讯飞 SDK。
//!
//! 行为：每累计 30s 音频（或 ≥ 30s 自上次唤醒以来的 wall clock），下一次 feed 返回
//! `Some(("玄女", 0.9))`。两次唤醒之间天然 ≥ 30s，满足 ≥ 1.5s 静音段去重契约。
//!
//! 选 wall clock 而非"累计字节数"是为了：联调时若客户端发包速率不准（移动网络抖、
//! 低优先级线程被挂起），按字节数算会把唤醒延迟到不可控时间；按 wall clock 给用户
//! 一个稳定预期"半分钟出一次"。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;

use super::WakeEngine;

/// Mock 唤醒间隔——30s。
pub const MOCK_INTERVAL: Duration = Duration::from_secs(30);

/// Mock 引擎。`last_wake` = 上次唤醒时刻（None = 还没首次 init）。
pub struct MockEngine {
    last_wake: Mutex<Option<Instant>>,
    /// 默认关键词；init 时被覆盖。
    keyword: Mutex<String>,
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEngine {
    pub fn new() -> Self {
        Self {
            last_wake: Mutex::new(None),
            keyword: Mutex::new("玄女".into()),
        }
    }
}

#[async_trait]
impl WakeEngine for MockEngine {
    async fn init(&self, keywords: &[String]) -> Result<()> {
        // init 时把锚点设为当前时刻——下一次唤醒在 30s 后。
        *self.last_wake.lock().expect("mock lock") = Some(Instant::now());
        if let Some(first) = keywords.first() {
            *self.keyword.lock().expect("mock lock") = first.clone();
        }
        Ok(())
    }

    async fn feed(&self, _pcm: &[u8]) -> Result<Option<(String, f32)>> {
        let mut last = self.last_wake.lock().expect("mock lock");
        let now = Instant::now();
        let due = match *last {
            Some(t) => now.duration_since(t) >= MOCK_INTERVAL,
            // 没 init 也允许喂——喂第一次起锚，不立刻命中。
            None => {
                *last = Some(now);
                false
            }
        };
        if due {
            *last = Some(now);
            let kw = self.keyword.lock().expect("mock lock").clone();
            Ok(Some((kw, 0.9)))
        } else {
            Ok(None)
        }
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn does_not_fire_immediately_after_init() {
        let m = MockEngine::new();
        m.init(&["玄女".into()]).await.unwrap();
        for _ in 0..5 {
            assert!(m.feed(&[0u8; 1280]).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn fires_after_interval() {
        let m = MockEngine::new();
        m.init(&["玄女".into()]).await.unwrap();
        // 手动把 last_wake 推到 31s 前——绕开真等 30s。
        {
            let mut lw = m.last_wake.lock().unwrap();
            *lw = Some(Instant::now() - Duration::from_secs(31));
        }
        let hit = m.feed(&[0u8; 1280]).await.unwrap();
        assert!(hit.is_some());
        let (kw, score) = hit.unwrap();
        assert_eq!(kw, "玄女");
        assert!(score > 0.0);
    }

    #[tokio::test]
    async fn dedupes_consecutive_fires_within_window() {
        // 命中后立刻再 feed 不应再命中——内部锚点已被推到当前时刻。
        let m = MockEngine::new();
        m.init(&["玄女".into()]).await.unwrap();
        {
            let mut lw = m.last_wake.lock().unwrap();
            *lw = Some(Instant::now() - Duration::from_secs(31));
        }
        assert!(m.feed(&[0u8; 1280]).await.unwrap().is_some());
        for _ in 0..5 {
            assert!(m.feed(&[0u8; 1280]).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn init_overrides_first_keyword() {
        let m = MockEngine::new();
        m.init(&["贾维斯".into()]).await.unwrap();
        {
            let mut lw = m.last_wake.lock().unwrap();
            *lw = Some(Instant::now() - Duration::from_secs(31));
        }
        let (kw, _) = m.feed(&[0u8; 1280]).await.unwrap().unwrap();
        assert_eq!(kw, "贾维斯");
    }
}
