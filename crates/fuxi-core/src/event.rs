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

/// 门客向玄女呈递的 deliverable 类别（Decision 13 §4 初版枚举）。
/// 决定了玄女审阅时的展示模板与优先级；后续可扩展，但**新加 variant
/// 必须同步更门客 system prompt 的触发指南**（避免门客发出"无法分类"的 deliverable）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverableKind {
    ResearchSummary,
    CodeChange,
    TestResult,
    DecisionRequest,
    ErrorBlock,
}

/// 远端 worker 心跳状态——`WorkerHeartbeatStateChanged.status` 的类型。
/// 用 enum 而非 String：误拼（`"alvie"`）编译期挂；TUI 渲染走 match 也能漏 case 报错。
/// `serde(rename_all = "snake_case")` 让 wire JSON 仍是 `"alive"` / `"stale"`，
/// 跨语言/跨进程行为不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    /// 心跳正常——controller 在 stale 阈值内最近一次收到该 worker 的心跳。
    Alive,
    /// 已被 sweep 标记失联——上一拍的 sweep_stale 命中后会发一条
    /// `WorkerStaleSwept`，controller 内部把 `last_published_status` 标 stale，
    /// 让 worker 恢复后下次心跳的 `status_flipped` 触发回 Alive 翻转事件。
    Stale,
}

impl WorkerStatus {
    /// 与 wire JSON 一致的 snake_case 字符串——TUI/log 渲染用。
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkerStatus::Alive => "alive",
            WorkerStatus::Stale => "stale",
        }
    }
}

impl std::fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
    /// 远端 worker node_id——controller `/dist/event` republish 前 set 此字段；
    /// `None` = 本地 controller 自己产出。给 TUI/firehose 区分本地 vs 远端使用。
    /// `serde(default)` 保留对老 SQLite payload / 老 wire JSON 的反序列化兼容；
    /// `skip_serializing_if = "Option::is_none"` 让本地事件 JSON 不带这个 key。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_node_id: Option<String>,
}

impl EventMeta {
    pub fn now() -> Self {
        Self {
            id: Uuid::new_v4(),
            at: Utc::now(),
            session: None,
            agent: None,
            task: None,
            source_node_id: None,
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
    // WHY 删除 TaskDelivered/TaskCancelled（M3.6 孤儿清理）：
    // 没有发布点；终态走 TaskStateChanged{to: Done|Cancelled} 一条线。
    TaskBlocked {
        reason: String,
    },
    /// task 从 Blocked 回到 Ready——玄女拿到授权（或其它外部信号）后发。
    /// `input` 可选：用户授权时附带的话（"同意"/"同意，但改成 X"/空 等）。
    TaskResumed {
        input: Option<String>,
    },

    // ── conversation / A2A messages ─────────────────────────
    // WHY 删除 MessageSent/MessageReceived（M3.6 孤儿清理）：
    // 设计早期占位，从未发布也未订阅；A2A 通信不走 EventKind。
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
        /// 用户消息里所有被 @ 的 agent_id（含 target 自身，前端约定）。
        ///
        /// v3 #N7'（spec `2026-04-26-im-tab-bar-task-thread-design.md`）加：
        /// 任务 thread composer 允许多 chip @，第一个为路由 target，其余仅
        /// mention 标记（v1 不实装 fan-out，留 v2.x）。
        ///
        /// `#[serde(default)]` 让老事件（无该字段）回放时反序列化得空 Vec，
        /// 维持向后兼容——SQLite WAL 已落地的事件不需要回填。
        #[serde(default)]
        mentions: Vec<AgentId>,
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
    // ── scheduling / triggers（更漏 M1.3）──────────────────
    /// 新 trigger 入库——候簿上登记了一条新条目。
    TriggerRegistered {
        id: String,
        kind: String,
        spec: serde_json::Value,
    },
    /// Trigger 到期/被外部事件命中——统一入口，`cause` 区分来源。
    ///
    /// `cause` ∈ `"scheduled" | "manual" | "webhook" | "fs"`，见 `fuxi-scheduler::FireCause`。
    TriggerFired {
        id: String,
        fired_at: DateTime<Utc>,
        cause: String,
    },
    /// Trigger 已派给某个 agent（通常是玄女），进入执行路径。
    TriggerDispatched {
        id: String,
        to_agent: AgentId,
    },
    /// 本次 fire 被跳过——去重 / 熔断 / 无人接单。`reason` 必填。
    TriggerSkipped {
        id: String,
        reason: String,
    },
    /// Trigger 执行失败——consecutive_failures 会 +1；连续到上限 trigger 被 pause。
    TriggerFailed {
        id: String,
        error: String,
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

    // ── deliverable 边界 nudge（Decision 13）─────────────────
    /// 门客主动呼叫玄女审阅 deliverable——B1 attention 模型下**唯一**会
    /// 触发玄女主动消费的事件（中间事件玄女默认 silent，公理 2 重新定义为
    /// "可查"而非"必读"）。`artifact_ref` 可空：纯摘要类（如 ResearchSummary）
    /// 可只附 `summary`；CodeChange 类应填 commit sha 或 diff path。
    AgentRequestReview {
        agent: AgentId,
        task: TaskId,
        deliverable_kind: DeliverableKind,
        summary: String,
        artifact_ref: Option<String>,
    },
    /// `AgentRequestReview` 玄女在限时内未消费——兜底事件（Decision 13 §代价 3）。
    /// 由门客侧 retry 层或玄女订阅层超时检测发出，让玄女批量补审 / 让门客
    /// 决定是否继续阻塞。`waited_for_ms` 用毫秒整数：跨语言 / JSON 友好，
    /// 比 `chrono::Duration` 的 ISO8601 串更直观。`original_event_id` 与
    /// `OrchestratorCcReceived::original_intervention_id` 同样用裸 Uuid，
    /// 不引入新 `EventId` 类型；事件 id 在 `EventMeta::id` 即是 Uuid。
    ReviewRequestTimeout {
        original_event_id: Uuid,
        agent: AgentId,
        task: TaskId,
        waited_for_ms: u64,
    },

    // ── 分布式拓扑（Phase 6 P6 topology/metrics）─────────────
    /// Worker 向 controller 注册（首次或重连均发）。
    /// `tags` 决定路由匹配，`max_concurrency` 给容量调度。
    /// 重连场景下 controller 的 inflight 不会清——register 是"声明能力"，
    /// 不是"清空运行时"，inflight 由 heartbeat/sweep 维护。
    WorkerRegistered {
        node_id: String,
        tags: Vec<String>,
        max_concurrency: u32,
    },
    /// Worker 心跳带来的状态变化——**采样而非全发**。
    /// 心跳 200ms × N worker 全发会百万级噪声；只在以下条件发：
    /// - `inflight_count` 与上次发布不同
    /// - `status` 翻转（`Alive` ↔ `Stale`，stale→alive 是"sweep 后又回来了"）
    ///
    /// `status` 用 `WorkerStatus` enum 而非 String——类型安全 + match 漏 case
    /// 编译期报；wire JSON 仍是 `"alive"`/`"stale"`（`#[serde(rename_all)]`）。
    WorkerHeartbeatStateChanged {
        node_id: String,
        inflight_count: u32,
        status: WorkerStatus,
    },
    /// Sweep tick 把超时 worker 的 inflight job 回收到 global_queue 前端。
    /// `recycled_jobs` 可能为空（worker 死时本来就没活），但事件仍会发——
    /// 让 TUI 拓扑面板能感知"worker 失联"事件本身（而非只看 job 视角）。
    WorkerStaleSwept {
        node_id: String,
        recycled_jobs: Vec<String>,
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

    /// v3 #N7' `mentions` 字段加在 `UserInterventionSent` 上——
    /// 老事件（SQLite WAL 已落地）反序列化必须不挂，回出来 `mentions` 是空 Vec。
    /// `#[serde(default)]` 是这套 wire 兼容的契约。
    #[test]
    fn user_intervention_legacy_payload_without_mentions_deserializes() {
        // 老 wire 形态：无 mentions 字段（pre-#N7' 已落地的事件）
        let raw = serde_json::json!({
            "meta": {
                "id": "11111111-1111-1111-1111-111111111111",
                "at": "2026-04-26T10:00:00Z"
            },
            "kind": {
                "type": "user_intervention_sent",
                "target": "00000000-0000-0000-0000-000000000001",
                "mode": "append",
                "text": "old wire"
            }
        });
        let ev: Event = serde_json::from_value(raw).expect("legacy event");
        match ev.kind {
            EventKind::UserInterventionSent { mentions, mode, .. } => {
                assert!(mentions.is_empty(), "老事件 mentions 应回空 Vec");
                assert_eq!(mode, "append");
            }
            other => panic!("expect UserInterventionSent, got {other:?}"),
        }
    }

    /// v3 #N7' 完整 round-trip：mentions 数组写入 + 读出保留语义。
    #[test]
    fn user_intervention_with_mentions_roundtrip() {
        let target = AgentId::new();
        let other = AgentId::new();
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::UserInterventionSent {
                target,
                mode: "append".into(),
                text: "查 ERP-1066".into(),
                mentions: vec![target, other],
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("user_intervention_sent"));
        assert!(json.contains("mentions"));
        let back: Event = serde_json::from_str(&json).expect("de");
        match back.kind {
            EventKind::UserInterventionSent { mentions, .. } => {
                assert_eq!(mentions, vec![target, other]);
            }
            other => panic!("expect UserInterventionSent, got {other:?}"),
        }
    }

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
    fn agent_request_review_roundtrip() {
        let agent = AgentId::new();
        let task = TaskId::new();
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::AgentRequestReview {
                agent,
                task,
                deliverable_kind: DeliverableKind::CodeChange,
                summary: "已修 NULL 指针解引用".into(),
                artifact_ref: Some("commit:abc1234".into()),
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("agent_request_review"));
        assert!(json.contains("code_change"));
        let back: Event = serde_json::from_str(&json).expect("de");
        match back.kind {
            EventKind::AgentRequestReview {
                agent: a,
                task: t,
                deliverable_kind,
                summary,
                artifact_ref,
            } => {
                assert_eq!(a, agent);
                assert_eq!(t, task);
                assert_eq!(deliverable_kind, DeliverableKind::CodeChange);
                assert_eq!(summary, "已修 NULL 指针解引用");
                assert_eq!(artifact_ref.as_deref(), Some("commit:abc1234"));
            }
            other => panic!("不是 AgentRequestReview: {other:?}"),
        }
    }

    #[test]
    fn review_request_timeout_roundtrip() {
        let original = Uuid::new_v4();
        let agent = AgentId::new();
        let task = TaskId::new();
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::ReviewRequestTimeout {
                original_event_id: original,
                agent,
                task,
                waited_for_ms: 30_000,
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("review_request_timeout"));
        assert!(json.contains("30000"));
        let back: Event = serde_json::from_str(&json).expect("de");
        match back.kind {
            EventKind::ReviewRequestTimeout {
                original_event_id,
                agent: a,
                task: t,
                waited_for_ms,
            } => {
                assert_eq!(original_event_id, original);
                assert_eq!(a, agent);
                assert_eq!(t, task);
                assert_eq!(waited_for_ms, 30_000);
            }
            other => panic!("不是 ReviewRequestTimeout: {other:?}"),
        }
    }

    #[test]
    fn deliverable_kind_serde_snake_case() {
        for (kind, expect) in [
            (DeliverableKind::ResearchSummary, "\"research_summary\""),
            (DeliverableKind::CodeChange, "\"code_change\""),
            (DeliverableKind::TestResult, "\"test_result\""),
            (DeliverableKind::DecisionRequest, "\"decision_request\""),
            (DeliverableKind::ErrorBlock, "\"error_block\""),
        ] {
            let json = serde_json::to_string(&kind).expect("ser");
            assert_eq!(json, expect);
            let back: DeliverableKind = serde_json::from_str(&json).expect("de");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn worker_status_serde_snake_case() {
        for (kind, expect) in [
            (WorkerStatus::Alive, "\"alive\""),
            (WorkerStatus::Stale, "\"stale\""),
        ] {
            let json = serde_json::to_string(&kind).expect("ser");
            assert_eq!(json, expect);
            let back: WorkerStatus = serde_json::from_str(&json).expect("de");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn worker_topology_tags_match_snake_case() {
        for (kind, expect) in [
            (
                EventKind::WorkerRegistered {
                    node_id: "n".into(),
                    tags: vec!["cc".into()],
                    max_concurrency: 2,
                },
                "worker_registered",
            ),
            (
                EventKind::WorkerHeartbeatStateChanged {
                    node_id: "n".into(),
                    inflight_count: 1,
                    status: WorkerStatus::Alive,
                },
                "worker_heartbeat_state_changed",
            ),
            (
                EventKind::WorkerStaleSwept {
                    node_id: "n".into(),
                    recycled_jobs: vec!["job-a".into()],
                },
                "worker_stale_swept",
            ),
        ] {
            let v = serde_json::to_value(&kind).expect("ser");
            assert_eq!(v.get("type").and_then(|x| x.as_str()), Some(expect));
            let back: EventKind = serde_json::from_value(v).expect("de");
            // round-trip: tag check 已够；字段保真在 dist 层 publish 测试 + persistence 测试覆盖
            let again = serde_json::to_value(&back).expect("re-ser");
            assert_eq!(again.get("type").and_then(|x| x.as_str()), Some(expect));
        }
    }

    /// 默认 None 时 wire JSON 不带 `source_node_id` key——
    /// 老 reader（不认这个字段）反序列化也不影响；
    /// 同时本地事件的 payload 不被新字段污染。
    #[test]
    fn event_meta_source_node_id_omitted_when_none() {
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::PlatformStarted {
                version: "0.1".into(),
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(
            !json.contains("source_node_id"),
            "本地事件不应输出 source_node_id key, json={json}"
        );
    }

    /// 设置 Some(node_id) 后 wire JSON 带 key + roundtrip 字段保真。
    #[test]
    fn event_meta_source_node_id_roundtrip_when_some() {
        let mut meta = EventMeta::now();
        meta.source_node_id = Some("far".into());
        let ev = Event {
            meta,
            kind: EventKind::AgentResponded {
                text: "远端来的".into(),
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("\"source_node_id\":\"far\""), "json={json}");
        let back: Event = serde_json::from_str(&json).expect("de");
        assert_eq!(back.meta.source_node_id.as_deref(), Some("far"));
    }

    /// 老 SQLite payload / 老 wire JSON 不带 `source_node_id` 字段，
    /// 反序列化必须 fall back 到 None 而不是报错——`#[serde(default)]` 兜底。
    #[test]
    fn event_meta_deserializes_legacy_payload_without_source_node_id() {
        // 模拟 P2 之前生成的 payload：完全没有 source_node_id key。
        let legacy = serde_json::json!({
            "meta": {
                "id": Uuid::new_v4().to_string(),
                "at": Utc::now().to_rfc3339(),
                "session": null,
                "agent": null,
                "task": null,
            },
            "kind": {
                "type": "platform_started",
                "version": "0.0.1",
            }
        });
        let ev: Event = serde_json::from_value(legacy).expect("legacy de");
        assert!(ev.meta.source_node_id.is_none());
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
