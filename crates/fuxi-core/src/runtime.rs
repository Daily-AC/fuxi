//! Runtime trait — owns the process/container lifecycle of an agent.
//!
//! Borrowed from ComposioHQ's `Runtime` interface (types.ts:391) but
//! reshaped for Rust: handles are opaque, methods are async.

use crate::id::AgentId;
use crate::{AgentProfile, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeHandle {
    pub agent: AgentId,
    pub runtime_name: String,
    /// Implementation-specific opaque state (PID, container id, …).
    pub data: serde_json::Value,
    /// Where the agent's working files live.
    pub workdir: PathBuf,
}

#[async_trait]
pub trait Runtime: Send + Sync {
    fn name(&self) -> &str;

    async fn create(&self, profile: &AgentProfile, workdir: PathBuf) -> Result<RuntimeHandle>;
    async fn destroy(&self, handle: &RuntimeHandle) -> Result<()>;
    async fn is_alive(&self, handle: &RuntimeHandle) -> Result<bool>;
}
