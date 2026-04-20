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
    /// 召回复用入口：true 表示这个 handle 只是"借用现有 worktree"——
    /// 不是 fuxi 创建的；destroy 时必须跳过 `git worktree remove` 和 `branch -D`，
    /// 否则会把仍可能被其他召回引用的 worktree 干掉。
    /// 默认 false 兼容旧 serde 数据。
    #[serde(default)]
    pub borrowed: bool,
}

#[async_trait]
pub trait Workspace: Send + Sync {
    async fn create(&self, agent: AgentId, base_branch: &str) -> Result<WorkspaceHandle>;
    async fn destroy(&self, handle: &WorkspaceHandle) -> Result<()>;
    async fn list(&self) -> Result<Vec<WorkspaceHandle>>;
}
