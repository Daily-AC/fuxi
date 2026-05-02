//! 伏羲 worktree 隔离层。
//!
//! 这个 crate 实现 [`fuxi_core::Workspace`] trait：把一个 git 仓库切成多个
//! **独立 worktree**——每个门客（Dev agent）占一个分支 + 一个工作目录，
//! 彼此不踩脚。对应设计文档 §2 锚点场景步骤 6（两个 Dev 并行）和 §6 `Workspace` 行。
//!
//! 使用形态：
//! ```ignore
//! use fuxi_workspace::GitWorktreeWorkspace;
//! use fuxi_core::{Workspace, id::AgentId};
//!
//! # async fn demo() -> fuxi_core::Result<()> {
//! let ws = GitWorktreeWorkspace::with_default_base("/path/to/repo");
//! let handle = ws.create(AgentId::new(), "main").await?;
//! // ... agent 在 handle.worktree_path 里干活 ...
//! ws.destroy(&handle).await?;
//! # Ok(()) }
//! ```
//!
//! 公理对齐：
//! - 公理 #5：`list()` 永远以 `git worktree list --porcelain` 为真相源，
//!   内存 registry 只是缓存。
//! - 不在库里 `unwrap()`——所有失败路径返回 [`WorkspaceError`]。

mod git;
pub mod porcelain;
pub mod project_registry;

use std::path::PathBuf;

pub use git::GitWorktreeWorkspace;
pub use porcelain::{PorcelainWorktree, parse as parse_porcelain};
pub use project_registry::FileSystemProjectRegistry;

/// 本 crate 的错误类型。
///
/// WHY：`fuxi_core::CoreError` 没有 git 语义的 variant；我们在本地
/// 精细表达，再通过 [`WorkspaceError::into_core`] / `From` 降级成
/// `CoreError::Other` 满足 trait 签名。
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// I/O 错误（建目录、读文件等）。
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// git 子命令非零退出。
    #[error("git command failed: `{command}`\n--- stderr ---\n{stderr}")]
    Git {
        /// 完整命令行，形如 `git worktree add -b ... <path> <base>`。
        command: String,
        /// git 进程的 stderr 原文（已从 UTF-8 lossy 化）。
        stderr: String,
    },

    /// 目标路径不是一个 git 工作树。
    #[error("not a git repository: {0}")]
    NotAGitRepo(PathBuf),

    /// worktree 目录已存在（不复用，避免踩到别人的未提交改动）。
    #[error("worktree path already exists: {0}")]
    AlreadyExists(PathBuf),

    /// 兜底杂项错误——带上下文字符串。
    #[error("{0}")]
    Other(String),
}

impl WorkspaceError {
    /// 把本 crate 错误降级成 `fuxi_core::CoreError`，用在 `Workspace` trait 实现里。
    ///
    /// WHY：trait 签名固定返回 `fuxi_core::Result`，但我们想保留 git 细节的可打印性；
    /// `Other(to_string)` 既不丢信息、又无需动 core 的错误枚举。
    pub fn into_core(self) -> fuxi_core::CoreError {
        fuxi_core::CoreError::Other(self.to_string())
    }
}

impl From<WorkspaceError> for fuxi_core::CoreError {
    fn from(e: WorkspaceError) -> Self {
        e.into_core()
    }
}
