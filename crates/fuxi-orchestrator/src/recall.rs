//! `RecallSink`——dispatch pump 把 task 完成时的可重现 spawn context 入策府的钩子。
//!
//! WHY trait 在 orchestrator 而非 fuxi-memory：trait 必须在调用方 crate 才能被
//! `Fuxi` 持有；具体 impl（写 oracle）由 fuxi-cli 的 adapter 提供——和
//! `FactExtractorSpawner` 走同一个反向依赖 pattern（见 `fuxi-cli/src/extractor_hook.rs`）。
//!
//! ## 为什么是 RecallContext 而不是单 session_id
//!
//! P2 召回首版（commit `8feb7eb`）trait 是 `record_task_session(.., session_id: String)`。
//! 档 3 e2e 揭露根因：cc session 文件按 cwd 索引，门客每次 spawn 新 worktree → cwd
//! 不同 → resume 必死。**召回的真实最小依赖** = "整个工作环境"——cwd（worktree）
//! + cli-specific session if any。
//!
//! 见 `docs/decisions/07-recall-scope.md`。codex/gemini 的"召回"语义也通过本 context
//! 的 worktree 字段统一承载——前者无 cli_session_id（spawn-per-dispatch 无持久 session），
//! 后者将来 verbatim 加新字段即可，无需破坏 trait 签名。
//!
//! ## 语义：best-effort，错误只 warn 不传播
//!
//! 召回入库失败不该让 task 完成报错。dispatch pump 在 `TaskStateChanged { to: Done }`
//! 时调；**仅 Done**——Cancelled/Blocked 不入库（避免脏数据被召回拉出）。

use async_trait::async_trait;
use fuxi_core::id::{AgentId, TaskId};
use std::path::PathBuf;

/// 一次召回入库需要的全部 context——pump 收齐后整包传给 sink。
///
/// 字段语义：
/// - `agent_id` / `task_id`：标识源
/// - `role`：用于 `--recall-role` 的 query key（pump 从 shelf 反查）
/// - `worktree`：cwd 复用入口；None = 该 agent 没分 worktree（玄女这种）
/// - `cli_session_id`：cc 才有；codex 永远 None；gemini 视实现可能有
#[derive(Debug, Clone)]
pub struct RecallContext {
    pub agent_id: AgentId,
    pub task_id: TaskId,
    pub role: String,
    pub worktree: Option<PathBuf>,
    pub cli_session_id: Option<String>,
}

/// 把 task 完成时的 spawn context 入策府的钩子。
///
/// 不再以 cli_session_id 作为入库守门——pump 总是调 sink，sink 自行决定
/// 哪些字段值得落 fact（cc 写 session_id + worktree；codex 只写 worktree）。
#[async_trait]
pub trait RecallSink: Send + Sync {
    async fn record(&self, ctx: RecallContext);
}
