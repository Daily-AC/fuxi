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
/// 调用契约：调用方传 `topic_id`，impl 负责：
/// 1. 等当前玄女 turn idle（≤ 60s 超时强 kill）
/// 2. shutdown_xuannv_for_handoff(reason="topic_switch:...")
/// 3. 拼新 topic 的 prelude（最近 N 条对话回顾 / 进行中 task 摘要）
/// 4. spawn 新 cc 注 prelude → set_xuannv + set_current_topic
/// 5. touch_last_active(topic_id)
///
/// 失败语义：spawn 新副本失败应 bail——调用方按需 retry（HTTP 5xx）。
/// kill 老玄女失败应 warn 继续（老进程可能已死）。
#[async_trait]
pub trait XuannvSwitcher: Send + Sync {
    async fn switch_topic(&self, topic_id: TopicId) -> Result<()>;
}
