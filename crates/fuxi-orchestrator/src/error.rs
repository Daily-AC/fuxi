//! 编排层错误。
//!
//! WHY 独立错误类型：`Fuxi` 的错误来源混合——core/events/workspace/agent-cc
//! 各有各的错误；在这里统一一层，调用方（CLI）只要对接一个 `OrchestratorError`
//! 就行，不需要把五个依赖的错误类型都挂在自己的 enum 上。

use fuxi_core::id::AgentId;

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    /// 找不到指定门客。
    #[error("agent 未注册: {0}")]
    AgentNotFound(AgentId),

    /// core 层错误透传。
    #[error("core: {0}")]
    Core(#[from] fuxi_core::CoreError),

    /// events 层错误透传。
    #[error("events: {0}")]
    Events(#[from] fuxi_events::Error),

    /// workspace 层错误透传。
    #[error("workspace: {0}")]
    Workspace(#[from] fuxi_workspace::WorkspaceError),

    /// I/O——spawn 子进程等。
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// 兜底。
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, OrchestratorError>;
