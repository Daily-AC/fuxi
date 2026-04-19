//! Event types emitted into the EventBus.
//!
//! This is 伏羲's single source-of-truth vocabulary for "what just happened".
//! Every Firehose subscriber, world-model watcher, and audit-log writer
//! consumes these.
//!
//! Inspired by ComposioHQ's 30+ `OrchestratorEvent` variants — reshaped as
//! a Rust enum so exhaustive matches are compile-checked.

use crate::id::{AgentId, SessionId, TaskId};
use crate::task::TaskState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An event is always `{ meta, kind }`—meta is how the bus locates it,
/// kind is what happened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub meta: EventMeta,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMeta {
    /// Monotonically-ish event id (UUIDv7 later; v4 today).
    pub id: Uuid,
    pub at: DateTime<Utc>,
    /// Which session this event belongs to (usually = conversation thread).
    pub session: Option<SessionId>,
    /// Which agent produced it. `None` = platform-originated.
    pub agent: Option<AgentId>,
    /// Which task it relates to, if any.
    pub task: Option<TaskId>,
}

impl EventMeta {
    pub fn now() -> Self {
        Self {
            id: Uuid::new_v4(),
            at: Utc::now(),
            session: None,
            agent: None,
            task: None,
        }
    }
}

/// The event vocabulary. Add variants here; do NOT stringify event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    // ── agent lifecycle ─────────────────────────────────────
    AgentSpawning {
        role: String,
        cli: String,
    },
    AgentReady {
        endpoint: String,
    },
    AgentShuttingDown {
        reason: String,
    },
    AgentDead {
        cause: String,
    },

    // ── task lifecycle ──────────────────────────────────────
    TaskCreated {
        title: String,
        description: String,
    },
    TaskDispatched {
        to: AgentId,
    },
    TaskStateChanged {
        from: TaskState,
        to: TaskState,
    },
    TaskDelivered {
        artifact: serde_json::Value,
    },
    TaskBlocked {
        reason: String,
    },
    /// task 从 Blocked 回到 Ready——玄女拿到授权（或其它外部信号）后发。
    /// `input` 可选：用户授权时附带的话（"同意"/"同意，但改成 X"/空 等）。
    TaskResumed {
        input: Option<String>,
    },
    TaskCancelled {
        reason: String,
    },

    // ── conversation / A2A messages ─────────────────────────
    MessageSent {
        from: AgentId,
        to: AgentId,
        text: String,
    },
    MessageReceived {
        from: AgentId,
        text: String,
    },
    UserPrompted {
        text: String,
    },
    AgentResponded {
        text: String,
    },
    ThinkingStarted,
    ThinkingFinished,

    // ── tool use (opt-in granularity) ───────────────────────
    ToolCallStarted {
        tool: String,
        args: serde_json::Value,
    },
    ToolCallFinished {
        tool: String,
        ok: bool,
        output_preview: String,
    },

    // ── intervention / supervision (伏羲独创) ────────────────
    UserInterventionSent {
        target: AgentId,
        /// `append` / `interrupt`——区分两种介入模式（v0.1 薄片 I）。
        mode: String,
        text: String,
    },
    /// 门客因介入被打断当前 turn（`control_request/interrupt` 已送达）。
    /// 仅在 `mode=interrupt` 的介入路径上发。
    AgentInterrupted {
        reason: String,
    },
    /// 介入已应用到任务——wire 层确认消息已发出/门客已打断。
    /// 标志是 v0.1 scenario 断言点 19。
    TaskInterventionApplied {
        mode: String,
    },
    OrchestratorCcReceived {
        from_user_to: AgentId,
        text: String,
    },
    ConversationTransferred {
        from: AgentId,
        to: AgentId,
        reason: String,
    },
    ConversationReturned {
        from: AgentId,
        to: AgentId,
        brief: Option<String>,
    },

    // ── platform ────────────────────────────────────────────
    PlatformStarted {
        version: String,
    },
    PlatformStopping,

    // ── escape hatch ────────────────────────────────────────
    /// For events not yet promoted to their own variant. Keep use to a
    /// minimum—prefer adding a typed variant.
    Custom {
        label: String,
        payload: serde_json::Value,
    },
}
