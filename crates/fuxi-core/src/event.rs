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
    /// 呈报（抄送）：用户直接对门客说话时，玄女同步收到副本。
    /// meta.agent 置为玄女 id（说明"这是给玄女的信"）。
    /// `original_intervention_id` 关联到用户原 `UserInterventionSent` 的 event id，
    /// 方便 TUI/审计把两条消息串成一条链。
    OrchestratorCcReceived {
        from_user_to: AgentId,
        text: String,
        original_intervention_id: Uuid,
    },
    ConversationTransferred {
        from: AgentId,
        to: AgentId,
        reason: String,
    },
    /// 让贤（主对话权移交请求）：from 请求把主对话切到 to。
    /// 参照 langgraph `Command(goto, payload)`。TUI 订阅到此事件后把
    /// active target 切到 to；不阻塞 from。`brief` 是可选简报/交接上下文。
    ConversationHandoffRequested {
        from: AgentId,
        to: AgentId,
        reason: String,
        brief: Option<String>,
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

    // ── skills / 招贤 ────────────────────────────────────────
    /// 铸牒司产出的榜文已落到 `skills/<role>.staging/`。
    SkillStaged {
        role: String,
        template: String,
        path: String,
    },
    /// 玉牒审过 —— rename staging → active 完成。
    SkillApproved {
        role: String,
    },
    /// 榜文被驳回 —— staging 已清理。
    SkillRejected {
        role: String,
        reason: String,
    },
    /// 玉牒激活——订阅者可据此刷新 roster 或预热 runtime。
    SkillActivated {
        role: String,
    },
    /// 玄女发 —— 现有 role 不足，需要招贤。触发铸牒司。
    NoRoleMatched {
        need: String,
    },

    // ── escape hatch ────────────────────────────────────────
    /// For events not yet promoted to their own variant. Keep use to a
    /// minimum—prefer adding a typed variant.
    Custom {
        label: String,
        payload: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_staged_roundtrip() {
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::SkillStaged {
                role: "painter".into(),
                template: "dev".into(),
                path: "/tmp/skills/painter.staging/SKILL.md".into(),
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("skill_staged"));
        let back: Event = serde_json::from_str(&json).expect("de");
        match back.kind {
            EventKind::SkillStaged { role, template, .. } => {
                assert_eq!(role, "painter");
                assert_eq!(template, "dev");
            }
            other => panic!("不是 SkillStaged: {other:?}"),
        }
    }

    #[test]
    fn no_role_matched_roundtrip() {
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::NoRoleMatched {
                need: "画图门客".into(),
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("no_role_matched"));
        assert!(json.contains("画图门客"));
        let back: Event = serde_json::from_str(&json).expect("de");
        matches!(back.kind, EventKind::NoRoleMatched { .. });
    }

    #[test]
    fn skill_lifecycle_tags_match_snake_case() {
        for (kind, expect) in [
            (
                EventKind::SkillStaged {
                    role: "x".into(),
                    template: "t".into(),
                    path: "p".into(),
                },
                "skill_staged",
            ),
            (
                EventKind::SkillApproved { role: "x".into() },
                "skill_approved",
            ),
            (
                EventKind::SkillRejected {
                    role: "x".into(),
                    reason: "bad".into(),
                },
                "skill_rejected",
            ),
            (
                EventKind::SkillActivated { role: "x".into() },
                "skill_activated",
            ),
        ] {
            let v = serde_json::to_value(&kind).expect("ser");
            assert_eq!(v.get("type").and_then(|x| x.as_str()), Some(expect));
        }
    }
}
