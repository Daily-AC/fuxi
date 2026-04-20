//! `RecallSink`——dispatch pump 把 task 完成时的 (task_id → cli session_id)
//! 入策府的注入点。
//!
//! WHY 在 orchestrator 而不是 fuxi-memory：trait 必须在调用方 crate 才能被
//! `Fuxi` 持有；具体 impl（写 oracle）由 fuxi-cli 的 adapter 提供——和
//! `FactExtractorSpawner` 走同一个反向依赖 pattern（见 `fuxi-cli/src/extractor_hook.rs`）。
//!
//! 语义：best-effort，错误只 warn 不传播——召回入库失败不该让 task 完成报错。

use async_trait::async_trait;
use fuxi_core::id::{AgentId, TaskId};

/// 把 task 完成时的 cli session_id 入策府的钩子。
///
/// dispatch pump 在 `TaskStateChanged { to: Done }` 时调；**仅 Done**——
/// Cancelled/Blocked 不入库（设计：召回基于完成态，避免脏数据）。
///
/// `agent_id` 必须带上：召回 shortcut `--recall-role <role>` 要查 `role-<role>`
/// 的 session，sink 实现需要从 agent_id 反查 role（一般经 Fuxi.list_workers）。
#[async_trait]
pub trait RecallSink: Send + Sync {
    /// 记录一条 `task-<id> → session_id` 映射；实现方应同时落 `role-<role>`
    /// 给 `--recall-role` 走（query_one 取 updated_at DESC 最新）。
    async fn record_task_session(&self, agent_id: AgentId, task_id: TaskId, session_id: String);
}
