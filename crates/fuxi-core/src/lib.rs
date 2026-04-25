//! 伏羲 core traits and types.
//!
//! This crate is the vocabulary layer. Nothing here executes —
//! it defines what an Agent, Runtime, Event, and Task *are*, so
//! other crates can implement and compose them.

pub mod agent;
pub mod event;
pub mod id;
pub mod runtime;
pub mod task;
pub mod trigger_lookup;
pub mod workspace;

pub use agent::{Agent, AgentCard, AgentProfile, AgentStatus};
pub use event::{DeliverableKind, Event, EventKind, EventMeta};
pub use id::{AgentId, SessionId, TaskId};
pub use runtime::{Runtime, RuntimeHandle};
pub use task::{Task, TaskState};
pub use trigger_lookup::TriggerLookup;
pub use workspace::{Workspace, WorkspaceHandle};

/// Crate-wide error type. Downstream crates wrap this in their own
/// Error enum when they add domain-specific variants.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("agent not found: {0}")]
    AgentNotFound(AgentId),
    #[error("task not found: {0}")]
    TaskNotFound(TaskId),
    #[error("invalid state transition: {from:?} → {to:?}")]
    InvalidTransition { from: TaskState, to: TaskState },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
