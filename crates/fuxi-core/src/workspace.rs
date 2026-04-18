//! Workspace trait — owns git worktree creation/isolation/cleanup.
//!
//! Borrowed from ComposioHQ's `Workspace` interface (types.ts:601).

use crate::Result;
use crate::id::AgentId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceHandle {
    pub agent: AgentId,
    pub repo_root: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
}

#[async_trait]
pub trait Workspace: Send + Sync {
    async fn create(&self, agent: AgentId, base_branch: &str) -> Result<WorkspaceHandle>;
    async fn destroy(&self, handle: &WorkspaceHandle) -> Result<()>;
    async fn list(&self) -> Result<Vec<WorkspaceHandle>>;
}
