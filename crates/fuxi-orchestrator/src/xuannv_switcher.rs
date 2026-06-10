//! Phase 1 · `XuannvSwitcher`——切玄女当前 topic 的反向依赖入口。
//!
//! ## 为什么 trait 在 orchestrator，impl 在 fuxi-cli
//!
//! 切 topic 需要 spawn 新 cc 进程 + 注 prelude，spawn 链路依赖
//! `fuxi-skills` + `fuxi-agent-cc`——这些都在 fuxi-cli 才能装配齐全。
//! 但调用方（fuxi-im 的 `/api/topics/:id/switch` handler）只能调到
//! orchestrator 层（fuxi-im 不能依赖 fuxi-cli，会成循环）。
//!
//! 解法是反向依赖：trait 在 orchestrator（最小 vocab），impl 在 fuxi-cli
//! 由启动期注入 Fuxi。和 `RecallSink` / `DistEnqueuer` 同 pattern。

use async_trait::async_trait;
use fuxi_core::TopicId;

use crate::Result;

/// 切玄女当前 topic 的反向依赖入口。
///
/// 调用契约（Phase 2）：调用方传 `topic_id`，impl 负责：
/// 1. 验证 topic 存在（不存在拒切）
/// 2. `ensure_xuannv_for_topic(topic_id)`——池有活分身秒切；无则懒启动
///    （topic 过滤回顾 prelude + drain 持久队列）
/// 3. `set_current_topic(topic_id)` + `touch_last_active(topic_id)`
///
/// **不** kill 旧分身（留 idle_gc dormant）、**不**等旧分身 idle。
///
/// 失败语义：ensure 失败应 bail 且不 flip current_topic——调用方按需
/// retry（HTTP 5xx）。
#[async_trait]
pub trait XuannvSwitcher: Send + Sync {
    async fn switch_topic(&self, topic_id: TopicId) -> Result<()>;
}
