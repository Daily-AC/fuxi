//! Event types emitted into the EventBus.
//!
//! This is 伏羲's single source-of-truth vocabulary for "what just happened".
//! Every Firehose subscriber, world-model watcher, and audit-log writer
//! consumes these.
//!
//! Inspired by ComposioHQ's 30+ `OrchestratorEvent` variants — reshaped as
//! a Rust enum so exhaustive matches are compile-checked.

use crate::id::{AgentId, SessionId, TaskId};
use crate::project::ProjectId;
use crate::task::TaskState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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

/// Workspace 五层 lifecycle（Decision 21）—— wire JSON 用 snake_case。
///
/// L0 / L-1 不需要事件流（前者是无 workspace、后者是脚踏地临时目录），
/// 故 enum 只列 L1/L2/L3 三种带身份的层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLayer {
    L1ReadOnly,
    L2Ephemeral,
    L3Persistent,
}

/// Workspace 跨事件关联键——`<project>/<layer>/<handle>` 字符串形态。
///
/// 例：`erp/L3/luban`、`erp/L2/<task-uuid>`、`fuxi/L1/<session>`。
/// WHY 不拆 struct 字段：事件持久化 / firehose 渲染 / SQL 查询都最爱 stable
/// string；构造时用 helper 保形态对齐。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    /// L3 持久 sandbox：`<project>/L3/<role>`。
    pub fn l3(project: &ProjectId, role: &str) -> Self {
        Self(format!("{project}/L3/{role}"))
    }

    /// L2 ephemeral worktree：`<project>/L2/<task-uuid>`。
    pub fn l2(project: &ProjectId, task: TaskId) -> Self {
        Self(format!("{project}/L2/{}", task.0))
    }

    /// L1 read-only mount：`<project>/L1/<handle>`。handle 由调用方决定
    /// （session id / 任务关联、视实装方便）。
    pub fn l1(project: &ProjectId, handle: &str) -> Self {
        Self(format!("{project}/L1/{handle}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// L2 / L3 archive 原因——审计用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveReason {
    /// 关联 task 终态（Done / Cancelled / Delivering）→ workspace 归档
    TaskCompleted,
    /// L2 promote 成 L3 后，原 L2 sandbox 进归档
    Promoted,
    /// 用户 / 玄女 显式归档
    ManualArchive,
}

/// 配额维度——`WorkspaceQuotaExceeded.quota_kind` 用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaKind {
    DiskBytes,
    ConcurrentSandboxes,
}

/// 文件级 deliverable 的单文件元信息——`DeliverableProduced.files` 用。
///
/// sha256 给审计校验防篡改 / 重复检测；size_bytes 给配额 / UI 显示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverableFileMeta {
    /// 文件名（不含路径，相对于 deliverables/<task-id>/）。
    pub name: String,
    /// 文件内容 sha256，hex 小写。
    pub sha256: String,
    pub size_bytes: u64,
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
        /// 用户在 PWA composer 用 `@<node_id>` 显式 pin 到的 dist 节点（如
        /// `mac-local`）。
        ///
        /// β · #57（spec `2026-04-27-im-dist-接通-design.md` gap e）加：
        /// 玄女 dispatch 决策树看到此字段非空 → 走 dist enqueue 到指定节点；
        /// 否则按 `task.required_tags` 或 default home 路径。
        ///
        /// 老事件 `#[serde(default)]` 回放回 None，向后兼容。
        #[serde(default)]
        pinned_node: Option<String>,
        /// 用户随消息附带的上传文件 id 列表（PWA 阶段 3 附件管道）。
        ///
        /// id 引向 `~/.fuxi/im_uploads` 的 `uploads` 表行，前端取直链
        /// `GET /api/uploads/:id` 渲染缩略图；玄女这边 handler 也用 id 反查
        /// 文件名 + 绝对路径并 prepend 到 text，让 cc 能 Read 真图片做 vision。
        ///
        /// 老事件 `#[serde(default)]` 回放回空 Vec，向后兼容。
        #[serde(default)]
        attachments: Vec<String>,
        /// 系统注入来源标记 —— 非 None 时表示该 intervene 不是用户敲键盘发的，
        /// 而是 bridge / sentinel addendum 等内部路径转发的系统消息。
        ///
        /// 取值（snake_case，跟 wire 一致）：
        /// - `"review_request"` — bridge 把 `AgentRequestReview` 翻成 `[REVIEW_REQUEST]`
        ///   注入玄女
        /// - `"review_timeout"` — bridge 把 `ReviewRequestTimeout` 翻成 `[REVIEW_TIMEOUT]`
        ///   兜底注入
        /// - `"agent_dead"` — bridge 把门客 `AgentDead` 转「门客 X 已下线」
        /// - `"trigger_fired"` — bridge 把 `TriggerFired` 拼三段式 `[TRIGGER_FIRED ...]`
        /// - `"carbon_copy"` — bridge 把 `OrchestratorCcReceived` 翻成 `[CC]` 抄送
        ///
        /// PWA reducer 看到此字段非 None → 渲染成玄女侧的「系统消息」气泡（左侧灰底
        /// 加角标），而不是右侧 user bubble，避免把系统注入误展示成用户语录。
        ///
        /// 老事件（pre-2026-05-03）`#[serde(default)]` 回放回 None，向后兼容。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_origin: Option<String>,
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

    // ── task-scoped agent mailbox ─────────────────────────────
    /// 门客间消息入队。初版是审计型 mailbox：所有消息必须挂 task，后续投递、已读、
    /// 失败都用同一个 `message_id` 串起来；不允许绕过 EventBus 私连。
    AgentMessageQueued {
        message_id: Uuid,
        from: AgentId,
        to: AgentId,
        text: String,
        summary: Option<String>,
    },
    /// 消息已被传输层送到目标门客的 inbox/turn 队列。注意 delivered != read。
    AgentMessageDelivered {
        message_id: Uuid,
        from: AgentId,
        to: AgentId,
    },
    /// 目标门客已消费该消息。`reader` 通常等于 queued.to，单独字段便于审计异常。
    AgentMessageRead {
        message_id: Uuid,
        reader: AgentId,
    },
    /// 消息投递失败。失败也要留痕，避免“看起来发了，其实黑洞”。
    AgentMessageFailed {
        message_id: Uuid,
        from: AgentId,
        to: AgentId,
        error: String,
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

    // ── workspace lifecycle（Decision 21）─────────────────────
    /// 工作区物理创建——L1 mount / L2 worktree / L3 sandbox 建好时发。
    ///
    /// `role` 仅 L3 有意义（哪位门客的 sandbox），L2/L1 None。
    /// `task` 仅 L2 / L1 ephemeral 有意义，L3 None。
    /// `path` 是创建后的绝对路径（canonicalize 后）。
    WorkspaceCreated {
        workspace_id: WorkspaceId,
        project: ProjectId,
        layer: WorkspaceLayer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<TaskId>,
        path: PathBuf,
    },
    /// 工作区有写入——`files_changed` 是相对上次 mutated/commit 之后变化的
    /// 文件数（粗粒度，给审计用，不必精确到字节）。批量发布即可，不每次写都发。
    WorkspaceMutated {
        workspace_id: WorkspaceId,
        files_changed: u32,
    },
    /// 门客在 sandbox / ephemeral 里 commit 了。
    /// `commit_sha` 长 hex；`branch` 是写入的 branch 名。
    WorkspaceCommitted {
        workspace_id: WorkspaceId,
        commit_sha: String,
        branch: String,
    },
    /// 工作区进入归档区（不立即删，PWA 可见 24h）。
    WorkspaceArchived {
        workspace_id: WorkspaceId,
        reason: ArchiveReason,
    },
    /// 归档过期 GC 删除——审计兜底。
    WorkspaceCollected {
        workspace_id: WorkspaceId,
    },
    /// 配额超限——拒绝新建工作区时发，前端可据此提示用户清理 / 升 quota。
    WorkspaceQuotaExceeded {
        project: ProjectId,
        quota_kind: QuotaKind,
        requested: u64,
        limit: u64,
    },
    /// L2 ephemeral 被 promote 成 L3 持久 sandbox（用户 / 玄女判定值得继续）。
    /// `to_role` 是新 L3 sandbox 关联的门客角色。
    WorkspacePromoted {
        from_workspace_id: WorkspaceId,
        to_role: String,
        project: ProjectId,
    },

    // ── deliverables（Decision 22）────────────────────────────
    /// 门客交付一组文件——Decision 13 的 `AgentRequestReview.artifact_ref`
    /// 物理存储侧的对应事件。`deliverable_kind` 沿用 Decision 13 五类枚举。
    DeliverableProduced {
        task: TaskId,
        project: ProjectId,
        deliverable_kind: DeliverableKind,
        files: Vec<DeliverableFileMeta>,
    },
    /// 用户接收交付——`accepted_to` 非 None 表 Direct 模式 / 用户在 PWA 指定
    /// 落地路径；None 表保持在 deliverables/<task-id>/ 不外迁。
    DeliverableAccepted {
        task: TaskId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        accepted_to: Option<PathBuf>,
    },
    /// 用户拒绝交付。
    DeliverableRejected {
        task: TaskId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// 交付过期未处理被 GC——v1 default 是「永久保留」，本变体留给后续若开
    /// auto-GC 用。schema 现在就锁好，避免将来再走五处同步。
    DeliverableExpired {
        task: TaskId,
    },

    // ── lightweight inline file push（P2.7）─────────────────
    /// 门客把一段文字（通常是 markdown）直推到任务对话流——不进 deliverables
    /// 收件箱、不做物理文件落地。用户在 PWA 任务私聊页里看到一个 worker bubble
    /// 嵌入 md 预览的卡片。
    ///
    /// 与 `DeliverableProduced` 的产品分工：
    /// - `DeliverableProduced` = 产物 / 工件级，进收件箱、可 accept_to 落地、走五分类
    /// - `AgentInlineMessagePushed` = 通知/小报告，"我跑出来这个 markdown 你瞄一眼"
    ///
    /// `body` 上限 256KB（在 CLI 入口处校验），超限请走 deliverable。
    /// `mime` 当前只承认 `text/markdown` / `text/plain`——前端按 mime 决定是否渲染 md。
    AgentInlineMessagePushed {
        task: TaskId,
        from: AgentId,
        /// 用户可见的文件名（basename，前端展示用）
        filename: String,
        /// `text/markdown` / `text/plain`
        mime: String,
        /// 文件正文，UTF-8。CLI 校验 ≤ 256KB。
        body: String,
    },

    // ── 玄女上下文管理（task #8 / Decision 11）───────────────
    /// 一次 LLM turn 结束后由 cc adapter 翻 result.usage 发出。`total_tokens`
    /// 故意 = `input + cache_creation + output`，**不**算 `cache_read`——
    /// cache_read 是命中 prefix cache 不占新 context window。`window_size` 是
    /// 模型 context window 上限（如 1m ≈ 1_000_000）；`pct` = total/window 给
    /// 订阅者免重算。
    ///
    /// 玄女自己的 turn 才参与上下文累加；其他门客的 UsageReport 只是审计材料
    /// （未来给单 turn 成本核算 / 长 task 总耗用展示用）。
    UsageReport {
        input_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        output_tokens: u64,
        /// = input + cache_creation + output。Adapter 算好上发，避免订阅者重算。
        total_tokens: u64,
        window_size: u64,
        /// 0.0 ~ 1.0。Adapter 算好；订阅者比阈值时直接用。
        pct: f32,
    },
    /// 玄女上下文跨过水位（35% / 45% / ...）触发的水位标记事件。
    /// 同一 spawn 周期内每个阈值最多发一次（订阅者用阈值反查，不重复触发）。
    /// `threshold_pct` 是按整数百分比记的阈值（35 / 45）方便日志可读。
    XuannvContextWatermark {
        threshold_pct: u8,
        total_tokens: u64,
        window_size: u64,
        /// "addendum"（35%）/ "handoff_offer"（45%）—— 描述本次水位的动作类别。
        action: String,
    },
    /// 玄女或用户已写下 handoff 文件——后端检测到此事件后，等当前 turn idle
    /// 即 kill old + spawn new + 注入 prelude。`path` 是 handoff markdown 文件
    /// 绝对路径；`length_chars` 给审计/日志。
    XuannvHandoffWritten {
        path: PathBuf,
        length_chars: u32,
    },
    /// 玄女在语音模式下显式想"念出来给用户听"的一句话。由玄女通过
    /// `fuxi xuannv say "..."` Bash 子命令触发，CLI 写 EventBus，订阅者
    /// （macOS Jarvis App）拿到后调系统 TTS 播。文字本身仍走 IM 正常对话流，
    /// 这条事件只是"语音侧的副本"，PWA 前端可忽略。
    ///
    /// 为何独立变体而不是在 `AgentResponded` 上塞 `speak: bool`：
    /// (1) 玄女回复 IM 是文字流（含 markdown / 代码 / 列表），整段念出来又长又啰嗦；
    /// (2) 玄女自己判断"哪句要念"语义清晰，工具调用粒度比响应粒度合适；
    /// (3) 独立 EventKind 让 App 端订阅与 PWA 解耦——PWA 不渲染该事件不影响展示。
    XuannvVoiceLine {
        /// 要念的文本，玄女自己控制简短（一两句）。CLI 不做硬上限——
        /// 极端长的内容由 TTS 端自己决定截断或拒绝。
        text: String,
        /// Phase 3 情绪映射：可选情绪标签，TTS 端按情绪换 ref audio + 桌宠
        /// 换 idle sprite mode。`None` 走默认 normal 兜底。
        /// `#[serde(default)]` 保 SQLite WAL 里老事件（无 emotion 字段）反序列化不挂。
        #[serde(default)]
        emotion: Option<String>,
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
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
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

    /// β · #57 `pinned_node` 字段加在 `UserInterventionSent` 上——
    /// 老事件（v3 #N7' 时只有 mentions）反序列化必须不挂，回出来 `pinned_node`
    /// 是 None。`#[serde(default)]` 是这套 wire 兼容的契约。
    #[test]
    fn user_intervention_legacy_with_mentions_only_deserializes_pinned_none() {
        // 老 wire 形态：只有 mentions，无 pinned_node（pre-#57 已落地的事件）
        let raw = serde_json::json!({
            "meta": {
                "id": "11111111-1111-1111-1111-111111111111",
                "at": "2026-04-26T10:00:00Z"
            },
            "kind": {
                "type": "user_intervention_sent",
                "target": "00000000-0000-0000-0000-000000000001",
                "mode": "append",
                "text": "old wire",
                "mentions": ["00000000-0000-0000-0000-000000000001"]
            }
        });
        let ev: Event = serde_json::from_value(raw).expect("legacy event");
        match ev.kind {
            EventKind::UserInterventionSent {
                pinned_node,
                mentions,
                ..
            } => {
                assert!(pinned_node.is_none(), "老事件 pinned_node 应回 None");
                assert_eq!(mentions.len(), 1);
            }
            other => panic!("expect UserInterventionSent, got {other:?}"),
        }
    }

    /// β · #57 完整 round-trip：pinned_node Some 写入 + 读出保留语义。
    #[test]
    fn user_intervention_with_pinned_node_roundtrip() {
        let target = AgentId::new();
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::UserInterventionSent {
                target,
                mode: "append".into(),
                text: "@mac-local 帮我 ls ~/erp".into(),
                mentions: vec![target],
                pinned_node: Some("mac-local".into()),
                attachments: Vec::new(),
                system_origin: None,
            },
        };
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("pinned_node"));
        assert!(json.contains("mac-local"));
        let back: Event = serde_json::from_str(&json).expect("de");
        match back.kind {
            EventKind::UserInterventionSent { pinned_node, .. } => {
                assert_eq!(pinned_node.as_deref(), Some("mac-local"));
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

    /// Decision 21 phase 1：workspace 7 个变体的 serde tag + roundtrip。
    /// 加新 EventKind 变体必须同步五处 + 加 roundtrip 测试做门禁。
    #[test]
    fn workspace_lifecycle_tags_match_snake_case() {
        let pid = ProjectId::new("erp").unwrap();
        let task = TaskId::new();
        let ws_l3 = WorkspaceId::l3(&pid, "luban");
        let ws_l2 = WorkspaceId::l2(&pid, task);

        for (kind, expect) in [
            (
                EventKind::WorkspaceCreated {
                    workspace_id: ws_l3.clone(),
                    project: pid.clone(),
                    layer: WorkspaceLayer::L3Persistent,
                    role: Some("luban".into()),
                    task: None,
                    path: PathBuf::from("/tmp/erp/sandboxes/luban"),
                },
                "workspace_created",
            ),
            (
                EventKind::WorkspaceMutated {
                    workspace_id: ws_l3.clone(),
                    files_changed: 3,
                },
                "workspace_mutated",
            ),
            (
                EventKind::WorkspaceCommitted {
                    workspace_id: ws_l3.clone(),
                    commit_sha: "abc1234567890".into(),
                    branch: "luban/erp-main".into(),
                },
                "workspace_committed",
            ),
            (
                EventKind::WorkspaceArchived {
                    workspace_id: ws_l2.clone(),
                    reason: ArchiveReason::TaskCompleted,
                },
                "workspace_archived",
            ),
            (
                EventKind::WorkspaceCollected {
                    workspace_id: ws_l2.clone(),
                },
                "workspace_collected",
            ),
            (
                EventKind::WorkspaceQuotaExceeded {
                    project: pid.clone(),
                    quota_kind: QuotaKind::DiskBytes,
                    requested: 6_000_000_000,
                    limit: 5_000_000_000,
                },
                "workspace_quota_exceeded",
            ),
            (
                EventKind::WorkspacePromoted {
                    from_workspace_id: ws_l2,
                    to_role: "luban".into(),
                    project: pid.clone(),
                },
                "workspace_promoted",
            ),
        ] {
            let v = serde_json::to_value(&kind).expect("ser");
            assert_eq!(v.get("type").and_then(|x| x.as_str()), Some(expect));
            // round-trip 通过——SQLite payload 反序列化不会挂
            let back: EventKind = serde_json::from_value(v).expect("de");
            let again = serde_json::to_value(&back).expect("re-ser");
            assert_eq!(again.get("type").and_then(|x| x.as_str()), Some(expect));
        }
    }

    /// Decision 22：deliverable 4 个变体的 serde tag + roundtrip。
    #[test]
    fn deliverable_tags_match_snake_case() {
        let task = TaskId::new();
        let pid = ProjectId::new("erp").unwrap();

        for (kind, expect) in [
            (
                EventKind::DeliverableProduced {
                    task,
                    project: pid.clone(),
                    deliverable_kind: DeliverableKind::ResearchSummary,
                    files: vec![DeliverableFileMeta {
                        name: "report.md".into(),
                        sha256: "abc123".into(),
                        size_bytes: 1024,
                    }],
                },
                "deliverable_produced",
            ),
            (
                EventKind::DeliverableAccepted {
                    task,
                    accepted_to: Some(PathBuf::from("/Users/e0_7/写作/2026-某文章.md")),
                },
                "deliverable_accepted",
            ),
            (
                EventKind::DeliverableRejected {
                    task,
                    reason: Some("内容不对".into()),
                },
                "deliverable_rejected",
            ),
            (
                EventKind::DeliverableExpired { task },
                "deliverable_expired",
            ),
        ] {
            let v = serde_json::to_value(&kind).expect("ser");
            assert_eq!(v.get("type").and_then(|x| x.as_str()), Some(expect));
            let back: EventKind = serde_json::from_value(v).expect("de");
            let again = serde_json::to_value(&back).expect("re-ser");
            assert_eq!(again.get("type").and_then(|x| x.as_str()), Some(expect));
        }
    }

    /// task #8 玄女上下文管理：UsageReport tag + 字段保真。`total_tokens`
    /// 故意 = `input + cache_creation + output`，**不**算 cache_read，
    /// 反回归保护「fuxi-agent-cc 算总数公式」的核心约定。
    #[test]
    fn usage_report_tag_and_total_excludes_cache_read() {
        let kind = EventKind::UsageReport {
            input_tokens: 1_000,
            cache_creation_tokens: 5_000,
            cache_read_tokens: 200_000,
            output_tokens: 500,
            total_tokens: 6_500, // = 1000 + 5000 + 500，不含 cache_read
            window_size: 1_000_000,
            pct: 0.0065,
        };
        let v = serde_json::to_value(&kind).expect("ser");
        assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("usage_report"));
        let back: EventKind = serde_json::from_value(v).expect("de");
        match back {
            EventKind::UsageReport {
                input_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                output_tokens,
                total_tokens,
                window_size,
                pct,
            } => {
                assert_eq!(input_tokens, 1_000);
                assert_eq!(cache_creation_tokens, 5_000);
                assert_eq!(cache_read_tokens, 200_000);
                assert_eq!(output_tokens, 500);
                assert_eq!(total_tokens, 6_500, "total 不含 cache_read");
                assert_eq!(window_size, 1_000_000);
                assert!((pct - 0.0065).abs() < 1e-6);
            }
            other => panic!("expect UsageReport, got {other:?}"),
        }
    }

    /// task #8：水位事件 35% addendum。
    #[test]
    fn xuannv_context_watermark_tag_and_roundtrip() {
        let kind = EventKind::XuannvContextWatermark {
            threshold_pct: 35,
            total_tokens: 350_000,
            window_size: 1_000_000,
            action: "addendum".into(),
        };
        let v = serde_json::to_value(&kind).expect("ser");
        assert_eq!(
            v.get("type").and_then(|x| x.as_str()),
            Some("xuannv_context_watermark")
        );
        let back: EventKind = serde_json::from_value(v).expect("de");
        match back {
            EventKind::XuannvContextWatermark {
                threshold_pct,
                total_tokens,
                window_size,
                action,
            } => {
                assert_eq!(threshold_pct, 35);
                assert_eq!(total_tokens, 350_000);
                assert_eq!(window_size, 1_000_000);
                assert_eq!(action, "addendum");
            }
            other => panic!("expect XuannvContextWatermark, got {other:?}"),
        }
    }

    /// task #8：HandoffWritten 事件 tag + path 保真。
    #[test]
    fn xuannv_handoff_written_tag_and_roundtrip() {
        let kind = EventKind::XuannvHandoffWritten {
            path: PathBuf::from("/Users/e0_7/.fuxi/xuannv-handoff.md"),
            length_chars: 412,
        };
        let v = serde_json::to_value(&kind).expect("ser");
        assert_eq!(
            v.get("type").and_then(|x| x.as_str()),
            Some("xuannv_handoff_written")
        );
        let back: EventKind = serde_json::from_value(v).expect("de");
        match back {
            EventKind::XuannvHandoffWritten { path, length_chars } => {
                assert_eq!(path, PathBuf::from("/Users/e0_7/.fuxi/xuannv-handoff.md"));
                assert_eq!(length_chars, 412);
            }
            other => panic!("expect XuannvHandoffWritten, got {other:?}"),
        }
    }

    /// P2.7 inline 文件推送：tag 一致 + body 完整往返。
    #[test]
    fn agent_inline_message_pushed_tag_and_roundtrip() {
        let task = TaskId::new();
        let from = AgentId::new();
        let kind = EventKind::AgentInlineMessagePushed {
            task,
            from,
            filename: "report.md".into(),
            mime: "text/markdown".into(),
            body: "# 报告\n\n你好世界".into(),
        };
        let v = serde_json::to_value(&kind).expect("ser");
        assert_eq!(
            v.get("type").and_then(|x| x.as_str()),
            Some("agent_inline_message_pushed")
        );
        let back: EventKind = serde_json::from_value(v).expect("de");
        match back {
            EventKind::AgentInlineMessagePushed {
                task: t,
                from: f,
                filename,
                mime,
                body,
            } => {
                assert_eq!(t, task);
                assert_eq!(f, from);
                assert_eq!(filename, "report.md");
                assert_eq!(mime, "text/markdown");
                assert_eq!(body, "# 报告\n\n你好世界");
            }
            other => panic!("expect AgentInlineMessagePushed, got {other:?}"),
        }
    }

    /// WorkspaceId 构造器对齐 Decision 21 的命名约定 `<project>/<layer>/<handle>`。
    #[test]
    fn workspace_id_helpers_format_consistently() {
        let pid = ProjectId::new("erp").unwrap();
        let task = TaskId::new();
        assert_eq!(WorkspaceId::l3(&pid, "luban").as_str(), "erp/L3/luban");
        let l2 = WorkspaceId::l2(&pid, task);
        assert!(l2.as_str().starts_with("erp/L2/"));
        assert_eq!(WorkspaceId::l1(&pid, "sess123").as_str(), "erp/L1/sess123");
    }

    /// Workspace/Deliverable 各 enum 的 snake_case wire 形态——给跨语言客户端
    /// 一个稳定契约。
    #[test]
    fn workspace_layer_serde_snake_case() {
        for (layer, expect) in [
            (WorkspaceLayer::L1ReadOnly, "\"l1_read_only\""),
            (WorkspaceLayer::L2Ephemeral, "\"l2_ephemeral\""),
            (WorkspaceLayer::L3Persistent, "\"l3_persistent\""),
        ] {
            assert_eq!(serde_json::to_string(&layer).unwrap(), expect);
        }
        for (reason, expect) in [
            (ArchiveReason::TaskCompleted, "\"task_completed\""),
            (ArchiveReason::Promoted, "\"promoted\""),
            (ArchiveReason::ManualArchive, "\"manual_archive\""),
        ] {
            assert_eq!(serde_json::to_string(&reason).unwrap(), expect);
        }
        for (qk, expect) in [
            (QuotaKind::DiskBytes, "\"disk_bytes\""),
            (QuotaKind::ConcurrentSandboxes, "\"concurrent_sandboxes\""),
        ] {
            assert_eq!(serde_json::to_string(&qk).unwrap(), expect);
        }
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

    /// Jarvis · 语音模式：玄女通过 `fuxi xuannv say` 触发 TTS 副本事件。
    /// tag = `xuannv_voice_line`，text 字段完整往返。
    #[test]
    fn xuannv_voice_line_tag_and_roundtrip() {
        let kind = EventKind::XuannvVoiceLine {
            text: "好的，已派给鲁班".into(),
            emotion: Some("happy".into()),
        };
        let v = serde_json::to_value(&kind).expect("ser");
        assert_eq!(
            v.get("type").and_then(|x| x.as_str()),
            Some("xuannv_voice_line")
        );
        assert_eq!(v.get("emotion").and_then(|x| x.as_str()), Some("happy"));
        let back: EventKind = serde_json::from_value(v).expect("de");
        match back {
            EventKind::XuannvVoiceLine { text, emotion } => {
                assert_eq!(text, "好的，已派给鲁班");
                assert_eq!(emotion.as_deref(), Some("happy"));
            }
            other => panic!("expect XuannvVoiceLine, got {other:?}"),
        }
    }

    /// Phase 3 加 `emotion` 字段后，老 wire（pre-emotion）反序列化必须不挂——
    /// `#[serde(default)]` 给 `emotion` 兜 `None`。
    #[test]
    fn xuannv_voice_line_legacy_payload_without_emotion_deserializes() {
        let raw = serde_json::json!({
            "type": "xuannv_voice_line",
            "text": "我在，你说。"
        });
        let kind: EventKind = serde_json::from_value(raw).expect("legacy de");
        match kind {
            EventKind::XuannvVoiceLine { text, emotion } => {
                assert_eq!(text, "我在，你说。");
                assert!(emotion.is_none(), "老事件 emotion 应回 None");
            }
            other => panic!("expect XuannvVoiceLine, got {other:?}"),
        }
    }
}
