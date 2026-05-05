//! `fuxi-orchestrator` · 玄女编排层——门客注册表 + spawn + dispatch。
//!
//! 这个 crate 把 `fuxi-core` 的 trait、`fuxi-events` 的总线、`fuxi-workspace`
//! 的 worktree 和具体门客适配器（当前 `fuxi-agent-cc`）粘合成一个可以被 CLI
//! 或未来 A2A server 直接拿来用的**最小可用玄女**：
//!
//! - **门客注册表**：按 `AgentId` 查询，记录 `AgentCard` + 状态 + 可选
//!   worktree handle。
//! - **spawn**：按 profile 拉起一个新门客，可选分配一个 worktree，并把相关
//!   平台事件（`AgentSpawning` / `AgentReady`）发到 EventBus。
//! - **dispatch**：拿到某门客、给它发 task；启动 pump 把 agent.dispatch
//!   返回的事件流 republish 到 EventBus（所有订阅者自动看到）。
//! - **dispatch_to_any**：legacy 兼容壳（内部转 task-bound spawn 语义）。
//!
//! **尚未做（留给 P2.5 / P3）**：
//! - 玄女自己作为一个 A2A server 暴露对外接口；
//! - 抄送式介入（InterventionProxy）；
//! - 主对话权转交（ConversationSwitch）；
//! - gemini / opencode 适配器纳入（codex 已接入）。

pub mod bridge;
pub mod dist_enqueuer;
pub mod error;
pub mod fuxi;
pub mod idle_gc;
pub mod insight_extractor;
pub mod mailbox;
pub mod recall;
pub mod registry;
pub mod sentinel_addendum;

pub use bridge::{Intervener, SystemEventBridge};
pub use dist_enqueuer::{DistEnqueueOptions, DistEnqueuer};
pub use error::{OrchestratorError, Result};
pub use fuxi::{Fuxi, FuxiConfig, WorkerKind};
pub use idle_gc::{
    DEFAULT_IDLE_TTL_SECS, DEFAULT_TICK_INTERVAL_SECS, IdleGcTask, IdleShutdowner, ttl_from_env,
};
pub use insight_extractor::{
    CANGJIE_ROLE, CANGJIE_SOURCE, CangjieSpawner, CangjieSpawnerError, CangjieSpawnerResult,
    DEFAULT_EXTRACT_TEMPLATE, DEFAULT_JUDGE_TEMPLATE, EXTRACTOR_ROLE, InsightExtractorConfig,
    InsightExtractorTask, config_from_env as insight_extractor_config_from_env,
};
pub use mailbox::{
    MailboxMessage, MailboxMessageState, fold_mailbox, mailbox_for_agent,
    mark_agent_message_delivered, mark_agent_message_failed, mark_agent_message_read,
    queue_agent_message,
};
pub use recall::{RecallContext, RecallSink};
pub use registry::{Shelf, ShelfEntry, ShelfStatus};
