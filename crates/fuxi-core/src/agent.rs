//! Agent trait + public metadata types.
//!
//! An `Agent` in 伏羲 is anything that can receive a task over A2A and
//! produce events. cc / codex / gemini-cli each implement this by wrapping
//! their CLI's headless mode.

use crate::id::{AgentId, TaskId};
use crate::{Event, Result, Task};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

/// 门客的角色档案——静态元信息。这是 "这个 agent 是谁、能干什么" 的声明。
/// 启发自 team-anya 的角色 profile。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Human-readable name like "pm-alpha" or "dev-frontend".
    pub name: String,
    /// Role label — "pm", "dev", "qa", "reviewer", or user-defined.
    pub role: String,
    /// Underlying CLI: "claude-code", "codex", "gemini-cli".
    pub cli: String,
    /// Role-specific system prompt snippet (will be assembled at launch).
    pub system_prompt: String,
    /// Free-form tags for discovery ("frontend", "rust", "research").
    pub tags: Vec<String>,
    /// Arbitrary extension fields.
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A2A Agent Card—公开能力声明，让其他 agent/platform 能发现并调用它。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub id: AgentId,
    pub profile: AgentProfile,
    /// A2A endpoint URL (e.g., "http://127.0.0.1:4101").
    pub endpoint: String,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Process alive, waiting for a task.
    Idle,
    /// Currently handling a task.
    Busy,
    /// Waiting for user input (e.g. PM is mid-conversation).
    AwaitingInput,
    /// Deliberately paused.
    Paused,
    /// Shutting down.
    Stopping,
    /// Dead.
    Dead,
}

/// 门客的运行时契约。由具体 agent adapter 实现（如 `xihe-agent-cc`）。
#[async_trait]
pub trait Agent: Send + Sync {
    fn card(&self) -> &AgentCard;

    /// Dispatch a task to the agent. Returns a receiver that streams events
    /// generated while the agent processes it.
    async fn dispatch(&self, task: Task) -> Result<mpsc::Receiver<Event>>;

    /// Send a freeform message mid-task (used by intervention &
    /// conversation handoff). The event stream from `dispatch` continues.
    async fn send_message(&self, task_id: TaskId, text: &str) -> Result<()>;

    /// Ask the agent to cancel its current work.
    async fn cancel(&self, task_id: TaskId) -> Result<()>;

    /// Tear the underlying process down.
    async fn shutdown(&self) -> Result<()>;
}
