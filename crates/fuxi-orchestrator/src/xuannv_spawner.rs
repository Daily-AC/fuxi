//! `XuannvSpawner`——按 topic 懒启动/重启玄女分身的钩子（块5 / task#7）。
//!
//! WHY trait 在 orchestrator 而非 fuxi-cli：[`crate::Fuxi::ensure_xuannv_for_topic`]
//! 和 bridge 的 dormant respawn 都住在 orchestrator，要在这里触发「为某 topic 起一只
//! 玄女分身」。但真 spawn 逻辑（拉该 topic 对话历史拼 prelude + cc launch）依赖
//! fuxi-im 的 conv_store + fuxi-cli 的 spawn_with_prelude——orchestrator **不能**依赖
//! 它们（循环）。故走依赖反转：trait 定在调用方，impl adapter 放 fuxi-cli。同
//! [`crate::PendingNotifySink`] / [`crate::RecallSink`] / `XuannvSwitcher` pattern。
//!
//! ## 语义
//!
//! `spawn_for_topic` 必须：spawn 一只**服务该 topic** 的玄女分身（注入「你服务
//! topic=X」addendum + 拉该 topic 历史 prelude）→ `set_xuannv_for_topic(topic, id)`
//! 入池 → 返回新 id。失败返 `Err`，调用方（ensure_xuannv_for_topic）按需 fallback
//! 或上抛。**绝不**给 cc 强塞 `--session-id`/`--resume`（CLAUDE.md 红线，让 cc 自生成）。

use crate::Result;
use fuxi_core::TopicId;
use fuxi_core::id::AgentId;

/// 为指定 topic 懒启动/重启玄女分身。
#[async_trait::async_trait]
pub trait XuannvSpawner: Send + Sync {
    /// spawn 一只服务 `topic` 的玄女分身，入池后返回其 id。幂等性由调用方
    /// （[`crate::Fuxi::ensure_xuannv_for_topic`] 先查池）保证；本方法被调时
    /// 视作「池里确实没有，去起一只」。
    async fn spawn_for_topic(&self, topic: TopicId) -> Result<AgentId>;
}
