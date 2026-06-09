//! `PendingNotifySink`——bridge 把「归属 topic 分身 dormant 时的完工/里程碑信号」
//! 落持久队列的钩子（a01cfab5「信号不丢」核心）。
//!
//! WHY trait 在 orchestrator 而非 fuxi-im：持久队列实现（`PendingNotifyStore`）住在
//! fuxi-im，而 fuxi-im **依赖** fuxi-orchestrator——orchestrator 反向依赖 fuxi-im 会
//! 循环。所以走依赖反转：trait 定在调用方（orchestrator），impl adapter 放顶层
//! fuxi-cli（同时看得见 `PendingNotifyStore` 和 `Fuxi`）。同 [`crate::RecallSink`] /
//! `fuxi-memory::FactExtractorSpawner`（见 `fuxi-cli/src/extractor_hook.rs`）pattern。
//!
//! ## 语义：best-effort
//!
//! 落库失败只 warn 不让 bridge 崩——但 enqueue 是「信号不丢」的最后一道，调用方
//! （bridge dormant 分支）应在失败时 warn 明确，让排错可见。

use crate::Result;
use fuxi_core::TopicId;

/// 把 dormant topic 的待补发通知落持久队列。
///
/// `topic_id`：归属 topic（分身 respawn 后按它 drain 补发）。
/// `prompt`：bridge 已组好的玄女注入文本（与活分身路径同一份 build_*_prompt 产物）。
/// `system_origin`：前端系统消息气泡 tag（`"review_request"` / `"agent_dead"` 等），
/// 块5 respawn 补发时透传给 `intervene_system` 保持气泡渲染一致。
#[async_trait::async_trait]
pub trait PendingNotifySink: Send + Sync {
    async fn enqueue(&self, topic_id: TopicId, prompt: &str, system_origin: &str) -> Result<()>;
}
