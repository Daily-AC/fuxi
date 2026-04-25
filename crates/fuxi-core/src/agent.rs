//! Agent trait + public metadata types.
//!
//! An `Agent` in 伏羲 is anything that can receive a task over A2A and
//! produce events. cc / codex / gemini-cli each implement this by wrapping
//! their CLI's headless mode.

use crate::event::DeliverableKind;
use crate::id::{AgentId, TaskId};
use crate::{CoreError, Event, Result, Task};
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

    /// 拿到底层 CLI session id（cc 的 `--resume` / 代理 resume 凭证）。
    ///
    /// WHY 默认 None：codex 是 spawn-per-dispatch 模式，没有持续 session；
    /// stub/test agent 也无须返。返 None 的语义是"该门客无可恢复 session 的概念"，
    /// 上层（dispatch pump）见 None 应 silent skip 召回入库，不当错误。
    ///
    /// WHY async：cc 的 session id 由 CLI 的 `system/init` 异步推过来，缓存在
    /// tokio Mutex 内（参见 `WsChannel::cli_session_id`）；同步暴露要么强加
    /// `std::sync::Mutex` 要么牵入 `OnceCell`，反而把适配层复杂化。
    async fn session_id(&self) -> Option<String> {
        None
    }

    /// 门客主动呼叫玄女审阅 deliverable（Decision 13）。**程序化 API**，与 LLM 在
    /// 输出里写 sentinel JSON 的"主路径"互补——后者由 adapter parser 直接翻译，
    /// 不经此方法。本方法供编排层 / 测试 / 未来工具从外部主动触发某门客发 review。
    ///
    /// **生产 adapter 必须 override**——default impl 仅供 stub/test。返 Err 不是
    /// fallback，是"该 adapter 不支持程序化 nudge，调用方该改走 dispatch 让门客
    /// 自己决定 nudge"。加新 adapter 忘了 override = silent logical bug（调用方
    /// 以为震过了实际没收到），所以宁可硬错。
    ///
    /// codex 适配器**不**override（保持默认 Err）：codex exec 是 spawn-per-dispatch，
    /// 无持久 active_tx，程序化推送无处可发；nudge 仅靠 LLM 输出 sentinel 经
    /// reader_loop 出去。
    async fn request_review(
        &self,
        _task_id: TaskId,
        _deliverable_kind: DeliverableKind,
        _summary: String,
        _artifact_ref: Option<String>,
    ) -> Result<()> {
        Err(CoreError::Other(
            "request_review not implemented for this adapter".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个最小 stub Agent——只为验证 trait 默认实装的行为。
    /// dispatch / send_message / cancel / shutdown 全 panic 不走（测试不调）。
    struct StubAgent {
        card: AgentCard,
    }

    #[async_trait]
    impl Agent for StubAgent {
        fn card(&self) -> &AgentCard {
            &self.card
        }
        async fn dispatch(&self, _t: Task) -> Result<mpsc::Receiver<Event>> {
            unreachable!()
        }
        async fn send_message(&self, _t: TaskId, _x: &str) -> Result<()> {
            unreachable!()
        }
        async fn cancel(&self, _t: TaskId) -> Result<()> {
            unreachable!()
        }
        async fn shutdown(&self) -> Result<()> {
            unreachable!()
        }
    }

    fn stub() -> StubAgent {
        StubAgent {
            card: AgentCard {
                id: AgentId::new(),
                profile: AgentProfile {
                    name: "stub".into(),
                    role: "stub".into(),
                    cli: "stub".into(),
                    system_prompt: String::new(),
                    tags: vec![],
                    extra: Default::default(),
                },
                endpoint: "stub".into(),
                status: AgentStatus::Idle,
            },
        }
    }

    /// trait 默认 `request_review` 必须返 `CoreError::Other`——这是 guardrail：
    /// 加新 adapter 忘了 override 时，调用方拿到硬错而非 silent ok。
    #[tokio::test]
    async fn request_review_default_returns_not_implemented_err() {
        let a = stub();
        let res = a
            .request_review(
                TaskId::new(),
                DeliverableKind::CodeChange,
                "stub summary".into(),
                None,
            )
            .await;
        let err = res.expect_err("default impl 必须 Err");
        let msg = format!("{err}");
        assert!(
            msg.contains("not implemented"),
            "错误文案要含 'not implemented' 便于上层日志识别，实测：{msg}"
        );
    }
}
