//! 系统事件 → 玄女唤醒桥（Decision 13 attention filter）。
//!
//! 订阅 EventBus，把"玄女该知情"的系统事件转成自然语言 prompt，通过
//! [`Intervener::intervene`] 追加到玄女对话。
//!
//! ## 白名单（触发 intervene 的事件类型）
//!
//! Decision 13 起从"广播一切"改为"白名单 push"——中间事件继续 publish
//! 进 EventBus（公理 #2 知情权 = 可查），但桥默认 silent，**只**这五类
//! 占用玄女 attention：
//!
//! - [`EventKind::TriggerFired`] —— 调度入口（cron / webhook）
//! - [`EventKind::AgentDead`] —— 门客失联兜底（非玄女 + 非 internal role）
//! - [`EventKind::OrchestratorCcReceived`] —— 用户→门客抄送
//! - [`EventKind::AgentRequestReview`] —— 门客主动 nudge（核心，B1 唯一推送通路）
//! - [`EventKind::ReviewRequestTimeout`] —— nudge 漏看后兜底
//!
//! 其余事件（AgentResponded / ToolCallStarted / ToolCallFinished /
//! TaskStateChanged 等）由 Firehose 渲染、SQLite 持久化、玄女想看
//! 自己 recall——但不主动 push。
//!
//! ## 公理对应
//! - #1 显式沟通 —— 白名单事件经 intervene 注入玄女对话，TUI 看不算到
//! - #2 知情权（重定义为"可查"）—— 中间事件依然全量入 EventBus + SQLite
//! - #3 真实时不轮询 —— 桥通过 `bus.subscribe()` 被动接收推送
//!
//! ## 为什么接 `OrchestratorCcReceived`（2026-04-20 修 Bug 7）
//!
//! 历史设计注释说"TUI 已订阅就够了"——**对人类坐在屏幕前成立，对 headless
//! 玄女不成立**。公理 #1「不显式沟通 = 没做」意味着：TUI 只是给用户看的
//! presentation，玄女 cc 实例的 stdin 没收到就是没收到。必须经过 intervene
//! 把抄送文本注入她下一轮对话。不会自环——`from_user_to` 这个事件字段已经
//! 说明 from 是 user、不是 xuannv，玄女对门客的 intervene 不触发此事件。
//!
//! ## 为什么不用 `Arc<Fuxi>` 做测试依赖
//!
//! `Fuxi` 实体需要 workspace + bus 等等复杂依赖。桥逻辑是纯事件 → prompt
//! 映射，抽 [`Intervener`] trait 让单测可以用 Mock 覆盖所有分支。

use crate::Result;
use crate::fuxi::Fuxi;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use fuxi_core::DeliverableKind;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::trigger_lookup::TriggerLookup;
use fuxi_events::EventBus;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// AgentRequestReview retry 退避序列（毫秒，指数退避）。
///
/// 暴露 `pub(crate)` 让单测 + delta 的 e2e fixture 直接复用——若 timeout
/// 预算调整，改这一处即可（delta 算 sum + buffer 作 timeout 等待预算）。
/// 当前 sum = 1700ms（200+500+1000），delta 用 2.5s 留余量。
pub(crate) const REVIEW_RETRY_BACKOFF_MS: &[u64] = &[200, 500, 1000];

/// 把固定 `AgentId` 包成 `watch::Receiver`（sender 立即 drop，receiver 仍能 borrow
/// 出最后值）。给非生产 spawn 路径（旧 `spawn_with` 兼容 + 单测）用——它们没有活的
/// `Fuxi` watch。生产 `spawn` 走 `fuxi.xuannv_id_watch()` 真随 respawn 漂移。
fn fixed_xuannv_watch(id: AgentId) -> watch::Receiver<Option<AgentId>> {
    let (tx, rx) = watch::channel(Some(id));
    drop(tx);
    rx
}

/// 内部 role 黑名单：这些门客的 [`EventKind::AgentDead`] **不抄送**给玄女，
/// 且 a58e45b4 引入的 TaskDone 兜底注入也对其静默。
///
/// 为什么：extractor / cangjie 都是平台后台自管理的"幕后工"——其生死与 task
/// 完结属系统层信号，玄女不应被这种噪音占 attention。
/// - extractor：M2.5 hook 自动 spawn/reap
/// - cangjie（issue eebe38ef）：insight extractor 短任务，每个 task done /
///   batch judge 都会派一只，bridge 之前把它当用户级 worker 抄送 "[TASK_DONE]
///   role=cangjie" 给玄女，是高频噪音。
const INTERNAL_ROLES: &[&str] = &["extractor", "cangjie"];

fn is_internal_role(role: &str) -> bool {
    INTERNAL_ROLES.contains(&role)
}

const BRIDGE_INTERRUPT_LAG_MS_DEFAULT: u64 = 3000;
const BRIDGE_INTERRUPT_MODE_DEFAULT: &str = "always";

fn parse_bool_token(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn bridge_interrupt_worker_reports() -> bool {
    // 默认开启：门客终态回报是高优先级信号，不应长期卡在玄女 busy 队列。
    // 若要回退追加式，可显式设 0/false/off。
    std::env::var("FUXI_BRIDGE_INTERRUPT_WORKER_REPORTS")
        .ok()
        .map(|v| parse_bool_token(&v))
        .unwrap_or(true)
}

fn bridge_interrupt_mode() -> String {
    std::env::var("FUXI_BRIDGE_INTERRUPT_MODE")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| BRIDGE_INTERRUPT_MODE_DEFAULT.to_string())
}

fn should_interrupt_worker_report(lag_ms: u64) -> bool {
    if !bridge_interrupt_worker_reports() {
        return false;
    }
    match bridge_interrupt_mode().as_str() {
        "lag" => lag_ms >= bridge_interrupt_lag_ms(),
        // 默认或未知值都按 always：门客终态回报优先于玄女当前忙碌对话
        _ => true,
    }
}

fn bridge_interrupt_lag_ms() -> u64 {
    std::env::var("FUXI_BRIDGE_INTERRUPT_LAG_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(BRIDGE_INTERRUPT_LAG_MS_DEFAULT)
}

fn bridge_delivery_lag_ms(at: DateTime<Utc>) -> u64 {
    let ms = Utc::now().signed_duration_since(at).num_milliseconds();
    ms.max(0) as u64
}

/// 桥需要的 orchestrator 能力——仅 `intervene` + `role_of`，方便测试用 Mock。
///
/// 生产实装由 [`Fuxi`] 提供；单测里用一个小 struct 覆盖即可。
#[async_trait]
pub trait Intervener: Send + Sync {
    async fn intervene(&self, agent_id: AgentId, interrupt_first: bool, text: &str) -> Result<()>;

    /// bug #76：bridge 注入系统消息时调这个，带 `system_origin` 标记
    /// （`"agent_dead"` / `"trigger_fired"` / `"review_request"` 等）让前端
    /// 渲染成玄女侧的「系统消息」气泡而不是右侧用户气泡。
    ///
    /// 默认实现退化到无标记的 `intervene`（单测 mock 不强求实现）；生产 Fuxi 覆盖。
    async fn intervene_system(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
        _system_origin: &str,
    ) -> Result<()> {
        self.intervene(agent_id, interrupt_first, text).await
    }

    /// 查门客当前登记的 role 标签——主要拿来给玄女读（"codex 门客下线"比裸 id 可读）。
    /// 未找到返回 None；调用方自行 fallback。
    async fn role_of(&self, agent_id: AgentId) -> Option<String>;

    /// 查门客的 worktree 路径——bridge 在 AgentRequestReview kind=code_change 时
    /// 反查 path 推断 WorkspaceId 给 WorkspaceCommitted 用。未注册 / 无 worktree
    /// → None。
    async fn worktree_of(&self, agent_id: AgentId) -> Option<std::path::PathBuf> {
        // 默认实现 None——单测 mock 不强求实现；生产 Fuxi 实装会覆盖。
        let _ = agent_id;
        None
    }

    /// Decision 21 phase 3：把指定 (project, task) 的 L2 ephemeral workspace
    /// 归档。bridge 在 AgentDead 时若识别出工作区是 L2 → 自动调本方法回收。
    /// 默认实现 silent Ok——单测 mock 不强求实现；生产 Fuxi 覆盖。
    async fn archive_l2_workspace(
        &self,
        project_id: fuxi_core::ProjectId,
        task: TaskId,
        reason: fuxi_core::ArchiveReason,
    ) -> Result<()> {
        let _ = (project_id, task, reason);
        Ok(())
    }

    /// Bug 修：task 终态（Done/Cancelled）时由 bridge 触发——按 task 反查所属
    /// project 的 L2 ephemeral 工作区，命中即归档。AgentDead 路径已存在但只
    /// 在门客死亡时生效；门客被 idle GC 走（不发 AgentDead）或 task 因别的
    /// 路径终结时，L2 永远不归档（用户实测 sia/L2/86106710 在 disk 躺 3 天）。
    /// 默认 silent Ok；生产 Fuxi 覆盖。
    async fn archive_l2_for_task(
        &self,
        task: TaskId,
        reason: fuxi_core::ArchiveReason,
    ) -> Result<()> {
        let _ = (task, reason);
        Ok(())
    }

    /// issue 1d816926：查 task 事件流里有没有真的产出过 `DeliverableProduced`。
    /// 用于强制校验门客 hallucinate 的「文件已交付 / apk 落 PWA」汇报——门客实际
    /// 没跑 `fuxi deliverable produce` 时事件流里没有这条，桥据此拦下，不让玄女
    /// 把幻觉当真转述给用户。
    ///
    /// 默认 `true`（视为已校验）——单测 mock 不强求实现、且「拿不到凭证就放行」比
    /// 「拿不到凭证就误拒合法交付」更安全；生产 Fuxi 覆盖走真查询。
    async fn has_deliverable_produced(&self, task: TaskId) -> bool {
        let _ = task;
        true
    }

    /// 块4：归属 topic 的分身 dormant 时，把完工/里程碑信号落持久队列
    /// （a01cfab5「信号不丢」）——分身 respawn 后 drain 补发（块5 收口）。
    ///
    /// 默认实现 debug 跳过——单玄女兼容期 / 测试 mock 不强求落库；生产 Fuxi 覆盖
    /// 走注入的 [`crate::PendingNotifySink`]（未注入时也 debug 跳过，不阻塞 bridge）。
    async fn enqueue_pending(
        &self,
        topic_id: fuxi_core::TopicId,
        prompt: &str,
        system_origin: &str,
    ) -> Result<()> {
        let _ = (topic_id, prompt, system_origin);
        debug!("enqueue_pending: 无 sink（默认实现），dormant 信号未落库");
        Ok(())
    }

    /// 块5：触发某 topic 的玄女分身懒启动/重启（dormant 补发用）。bridge 在归属
    /// 分身 dormant + enqueue 后调它——分身一起来就 drain 持久队列把刚入队的信号
    /// 补发，不必等用户下次开该 topic。
    ///
    /// 默认 no-op 返 None——单测 mock / 未注入 spawner 时不触发 respawn（信号仍在
    /// 队列里，等用户下次进该 topic 时 drain）；生产 Fuxi 覆盖走真 ensure。
    async fn ensure_xuannv_for_topic(&self, topic: fuxi_core::TopicId) -> Option<AgentId> {
        let _ = topic;
        None
    }
}

#[async_trait]
impl Intervener for Fuxi {
    async fn intervene(&self, agent_id: AgentId, interrupt_first: bool, text: &str) -> Result<()> {
        // 内部 bridge / 系统触发器走该 trait——不需要 @ mention / pinned_node /
        // attachments 语义，传空。只有 IM HTTP `POST /api/intervene` 走
        // Fuxi::intervene 直拨才会带 mentions / pinned_node / attachments。
        Fuxi::intervene(
            self,
            agent_id,
            interrupt_first,
            text,
            Vec::new(),
            None,
            Vec::new(),
            None,
        )
        .await
    }

    async fn intervene_system(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
        system_origin: &str,
    ) -> Result<()> {
        // bug #76：bridge 注入路径走这个，挂上 system_origin tag 让前端
        // 渲染玄女侧系统消息气泡。
        Fuxi::intervene_system_origin(
            self,
            agent_id,
            interrupt_first,
            text,
            system_origin.to_string(),
        )
        .await
    }

    async fn role_of(&self, agent_id: AgentId) -> Option<String> {
        // 本地 shelf 优先（最权威，含 live role）。
        if let Some(role) = self
            .list_workers()
            .await
            .into_iter()
            .find(|c| c.id == agent_id)
            .map(|c| c.profile.role.clone())
        {
            return Some(role);
        }
        // issue c63eb2ca · dist 路径：远端 worker 不在本地 shelf 但
        // AgentSpawning 已落 events.db（worker 端 publish + controller 透传）。
        // 走 events_by_kind 索引扫描——v1 spawning 事件总量小，毫秒级。
        // 跟 fuxi-im::tasks_view 的 role 兜底同款思路（#51）。
        let events = self
            .bus()
            .store()
            .events_by_kind("agent_spawning")
            .await
            .ok()?;
        events.into_iter().rev().find_map(|ev| {
            if ev.meta.agent != Some(agent_id) {
                return None;
            }
            match ev.kind {
                EventKind::AgentSpawning { role, .. } => Some(role),
                _ => None,
            }
        })
    }

    async fn worktree_of(&self, agent_id: AgentId) -> Option<std::path::PathBuf> {
        Fuxi::worktree_of(self, agent_id).await
    }

    async fn archive_l2_workspace(
        &self,
        project_id: fuxi_core::ProjectId,
        task: TaskId,
        reason: fuxi_core::ArchiveReason,
    ) -> Result<()> {
        Fuxi::archive_l2_workspace(self, project_id, task, reason).await
    }

    async fn archive_l2_for_task(
        &self,
        task: TaskId,
        reason: fuxi_core::ArchiveReason,
    ) -> Result<()> {
        Fuxi::archive_l2_for_task(self, task, reason).await
    }

    async fn has_deliverable_produced(&self, task: TaskId) -> bool {
        // 扫 task 完整历史找 DeliverableProduced。事件 append-only 且
        // DeliverableProduced（门客真跑 `fuxi deliverable produce` 时发）
        // 必早于 AgentRequestReview（完工 sentinel）入库，故此处一定看得到。
        // 查询失败按 true 放行——宁可漏拦也不误拒合法交付。
        match self.bus().history_for_task(task).await {
            Ok(events) => events
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::DeliverableProduced { .. })),
            Err(e) => {
                warn!(error = %e, %task, "has_deliverable_produced 查历史失败，按已校验放行");
                true
            }
        }
    }

    async fn enqueue_pending(
        &self,
        topic_id: fuxi_core::TopicId,
        prompt: &str,
        system_origin: &str,
    ) -> Result<()> {
        // 块4：转发给注入的持久队列 sink（依赖反转，impl 在 fuxi-cli）。未注入 =
        // 单玄女兼容期 / 测试，debug 跳过不阻塞 bridge——但生产**必须**注入，否则
        // dormant 分身的完工信号真丢（a01cfab5 回归）。
        let sink = self.pending_sink_handle().await;
        match sink {
            Some(s) => s.enqueue(topic_id, prompt, system_origin).await,
            None => {
                debug!(%topic_id, "enqueue_pending: pending_sink 未注入，dormant 信号未落库");
                Ok(())
            }
        }
    }

    async fn ensure_xuannv_for_topic(&self, topic: fuxi_core::TopicId) -> Option<AgentId> {
        // 块5：转发到 Fuxi 真懒启动（池有返回、miss 走注入的 XuannvSpawner respawn）。
        Fuxi::ensure_xuannv_for_topic(self, topic).await
    }
}

/// 从 worktree path 反查它属于哪个 L3 sandbox——仅 Decision 21 phase 1 路径
/// `<projects_root>/<project>/sandboxes/<role>/...` 形态命中。
///
/// 命中返 (WorkspaceId, branch)，未命中 None。沿用 deliverable produce 同款
/// 启发式（路径段两层 ancestor 匹配 sandboxes/<role> + 上一层 project 名）。
fn workspace_id_from_l3_path(path: &std::path::Path) -> Option<(fuxi_core::WorkspaceId, String)> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for dir in canon.ancestors() {
        let role = dir.file_name().and_then(|n| n.to_str())?;
        let parent = dir.parent()?;
        if parent.file_name().and_then(|n| n.to_str()) != Some("sandboxes") {
            continue;
        }
        let project_dir = parent.parent()?;
        let project_name = project_dir.file_name().and_then(|n| n.to_str())?;
        let project_id = fuxi_core::ProjectId::new(project_name.to_string()).ok()?;
        let workspace_id = fuxi_core::WorkspaceId::l3(&project_id, role);
        let branch = format!("{role}/{project_name}-main");
        return Some((workspace_id, branch));
    }
    None
}

/// Decision 21 phase 3：从 worktree path 反查 L2 ephemeral 归属——形如
/// `<projects_root>/<project>/ephemeral/<task-display>/...`。
///
/// 命中返 (ProjectId, TaskId)，未命中 None。task-display 形态 `task-<uuid>` 或
/// 裸 uuid 都接受（与 EphemeralWorkspaceManager::list_active 同款宽容）。
pub(crate) fn project_task_from_l2_path(
    path: &std::path::Path,
) -> Option<(fuxi_core::ProjectId, TaskId)> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for dir in canon.ancestors() {
        let task_seg = dir.file_name().and_then(|n| n.to_str())?;
        let parent = dir.parent()?;
        if parent.file_name().and_then(|n| n.to_str()) != Some("ephemeral") {
            continue;
        }
        let project_dir = parent.parent()?;
        let project_name = project_dir.file_name().and_then(|n| n.to_str())?;
        let trimmed = task_seg.strip_prefix("task-").unwrap_or(task_seg);
        let task_uuid = uuid::Uuid::parse_str(trimmed).ok()?;
        let project_id = fuxi_core::ProjectId::new(project_name.to_string()).ok()?;
        return Some((project_id, TaskId::from(task_uuid)));
    }
    None
}

/// 三段式 TriggerFired 唤醒 prompt——契约见 `docs/architecture-v1.md` §M1.3。
///
/// 和 `fuxi-scheduler::prompt::build_trigger_prompt` 保持字节级一致；fuxi-orchestrator
/// 不能依赖 fuxi-scheduler（会引入循环），所以这里各自持有一份。
fn build_trigger_prompt(id: &str, fired_at: DateTime<Utc>, cause: &str, intent: &str) -> String {
    format!(
        "[TRIGGER_FIRED id={id} fired_at={ts} cause={cause}]\n\n{intent}\n\n[INSTRUCTION: 判断当前环境是否适合执行此触发。适合则调度门客，不适合则回报原因]",
        ts = fired_at.to_rfc3339(),
    )
}

fn build_death_prompt(agent_id: AgentId, role: &str, cause: &str) -> String {
    format!("门客 {agent_id}（role={role}）已下线，原因：{cause}。请判断是否续派或告知用户。")
}

/// issue a58e45b4 · 门客 task done 兜底通知玄女——门客没主动跑
/// `_fuxi:request_review` sentinel 时的退路。措辞跟 AgentRequestReview 路径
/// 区分（"已完成 task" vs "呈递 deliverable 待审"），让玄女知道这是兜底
/// nudge 而非门客主动呈报。
fn build_task_done_prompt(agent_id: AgentId, role: &str, task_id: TaskId) -> String {
    format!(
        "[TASK_DONE] 门客 {agent_id}（role={role}）已完成 task {task_id}。\n\
         他没主动跑 `_fuxi:request_review` sentinel——你需要主动 `fuxi status --id {agent_id}` \
         看产物 / 摘要，必要时 `fuxi intervene` 让他跑 sentinel 把 deliverable 走流程。"
    )
}

fn build_cc_prompt(to_worker: AgentId, role: &str, text: &str) -> String {
    // bug #77：之前文案末「无需主动回话，除非判断需介入」给玄女留口子，cc 经常
    // 误判"需介入"主动反问用户「鲁班应答 ... 等你下一指令」。改硬规：抄送
    // **绝不**回话，仅记忆——除非用户后续明确追问"鲁班怎么样了"等。
    format!(
        "[CC · 仅知情] 用户直接对门客 {to_worker}（role={role}）说：「{text}」。\n\n\
         这是抄送你**只为留痕**——公理 #2 你有知情权但不可越权。**严禁**对此条主动回话或反问；\
         你的下一 turn 应直接 `idle`（不调任何工具、不发任何文字）。\n\
         只有用户后续显式追问鲁班状况时，再据此 CC 上下文回答。"
    )
}

fn deliverable_kind_tag(k: DeliverableKind) -> &'static str {
    // 与 EventKind 枚举 serde rename_all=snake_case 字面对齐——让玄女
    // prompt 里出现的标签和事件 JSON 里完全一致，便于跨视图（TUI / SQLite recall）
    // 用同一个搜索词关联同一笔 deliverable。
    match k {
        DeliverableKind::ResearchSummary => "research_summary",
        DeliverableKind::CodeChange => "code_change",
        DeliverableKind::TestResult => "test_result",
        DeliverableKind::DecisionRequest => "decision_request",
        DeliverableKind::ErrorBlock => "error_block",
        DeliverableKind::Artifact => "artifact",
    }
}

fn build_request_review_prompt(
    agent: AgentId,
    role: &str,
    kind: DeliverableKind,
    summary: &str,
    artifact_ref: Option<&str>,
) -> String {
    let mut prompt = format!(
        "[REVIEW_REQUEST] 门客 {agent}（role={role}）呈递 deliverable_kind={tag} 待审。\n\n摘要：{summary}",
        tag = deliverable_kind_tag(kind),
    );
    if let Some(r) = artifact_ref {
        prompt.push_str(&format!("\n\n附件：{r}"));
    }
    prompt.push_str(
        "\n\n[INSTRUCTION: 该门客主动找你审阅。判断是否接受 / 改派 / 让他续做，并向用户汇报或追问]",
    );
    prompt
}

/// issue 1d816926：门客呈递 artifact（apk 等文件）待审，但事件流里**没有**
/// 对应 `DeliverableProduced` 凭证——多半是 cc wind-down 阶段 hallucinate 了
/// 「已交付」却没真跑 `fuxi deliverable produce`。给玄女注入「未核实」警告，
/// 让她**别**按已交付转述给用户，而是打回门客真去交付。
fn build_unverified_deliverable_prompt(
    agent: AgentId,
    role: &str,
    summary: &str,
    artifact_ref: Option<&str>,
) -> String {
    let mut prompt = format!(
        "[REVIEW_REQUEST·未核实] 门客 {agent}（role={role}）呈递 deliverable_kind=artifact 待审，\
         但事件流里**没有** DeliverableProduced 凭证——他很可能只是嘴上说交付了，\
         实际没跑 `fuxi deliverable produce`，产物并未落到 PWA 收件箱。\n\n摘要：{summary}",
    );
    if let Some(r) = artifact_ref {
        prompt.push_str(&format!("\n\n附件（门客自称）：{r}"));
    }
    prompt.push_str(
        "\n\n[INSTRUCTION: 不要按「已交付」转述给用户——事件流无凭证。\
         `fuxi intervene --to <门客id>` 让他真去跑 `fuxi deliverable produce` 把文件交付，\
         核到 DeliverableProduced 事件后再向用户汇报。]",
    );
    prompt
}

fn build_review_timeout_prompt(
    agent: AgentId,
    role: &str,
    task: TaskId,
    waited_for_ms: u64,
) -> String {
    format!(
        "[REVIEW_TIMEOUT] 门客 {agent}（role={role}）的审阅请求超时未送达——已等 {waited_for_ms}ms（task={task}）。\n\n这通常意味着你前一段时间忙到忽略了门客 nudge。请回到该 task 现场补审，或主动 recall 该门客的最近事件了解进度。",
    )
}

/// AgentRequestReview 投递重试——首发 + 按 `backoff_ms` 序列指数退避 retry。
///
/// 全部失败时返 `Err`；调用方负责发 [`EventKind::ReviewRequestTimeout`] 兜底。
/// 拆出独立函数方便单测（生产传 [`REVIEW_RETRY_BACKOFF_MS`]，测试传 `&[1, 2, 4]` 等）。
pub(crate) async fn try_intervene_with_retry(
    intervener: &dyn Intervener,
    target: AgentId,
    prompt: &str,
    backoff_ms: &[u64],
    system_origin: &str,
) -> Result<()> {
    // 首发——若成功直接返回，不进 retry。bug #76：sentinel 注入走 intervene_system
    // 让 PWA 渲染玄女侧系统消息气泡。
    let mut last_err = match intervener
        .intervene_system(target, true, prompt, system_origin)
        .await
    {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    for (idx, &ms) in backoff_ms.iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        match intervener
            .intervene_system(target, true, prompt, system_origin)
            .await
        {
            Ok(()) => {
                debug!(retry = idx + 1, "review intervene retry succeeded");
                return Ok(());
            }
            Err(e) => {
                warn!(retry = idx + 1, error = %e, "review intervene retry failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// 系统事件 → 玄女唤醒桥。
pub struct SystemEventBridge;

impl SystemEventBridge {
    /// 生产路径——接入 `Arc<Fuxi>`。
    ///
    /// `trigger_lookup` 由 CLI 启动处注入（通常是 `Arc<TriggerStore>`），
    /// 避免 fuxi-orchestrator 直接依赖 fuxi-scheduler。
    ///
    /// Phase 1 #6：从 `fuxi.current_topic_watch()` 拿当前 topic receiver，
    /// handle_event 用它 filter 跨 topic 事件（meta.topic_id != current → silent 跳过，
    /// 公理 #2 保持知情权——非 milestone 事件不该污染当前玄女 prompt）。
    pub fn spawn(
        fuxi: Arc<Fuxi>,
        bus: EventBus,
        _xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        // 块3：生产路径改走**分身池路由**——每条事件按 `ev.meta.topic_id` 路由到
        // 归属 topic 的活分身，而非单玄女 + current_topic filter。pool_watch 实时跟随
        // respawn 漂移（同 feedback_dynamic_agent_id_via_watch）。pool 模式下不再用
        // current_topic filter / is_cross_topic_milestone 透传——每事件有明确 topic 归属。
        let pool_watch = fuxi.xuannv_pool_watch();
        // a01cfab5 修：玄女 id 在会话中会漂移；xuannv_watch 仍传作 general 兜底
        // （无 topic_id 的旧事件 / 平台级事件路由到 general 分身——它镜像在此 watch）。
        let xuannv_watch = fuxi.xuannv_id_watch();
        let intervener: Arc<dyn Intervener> = fuxi;
        Self::spawn_inner(
            intervener,
            bus,
            xuannv_watch,
            trigger_lookup,
            REVIEW_RETRY_BACKOFF_MS,
            None,
            Some(pool_watch),
        )
    }

    /// 测试/内部路径——任意 [`Intervener`] 注入。默认不带 topic filter（老行为
    /// 兼容），让 60+ test caller 不必逐一改签名。需要 filter 的测试用
    /// [`Self::spawn_with_topic`]。
    pub fn spawn_with(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        Self::spawn_inner(
            intervener,
            bus,
            fixed_xuannv_watch(xuannv_id),
            trigger_lookup,
            REVIEW_RETRY_BACKOFF_MS,
            None,
            None,
        )
    }

    /// Phase 1 #6 测试路径：带 topic filter 注入。`topic_watch` `Some` 时每条
    /// event 顶部判断 meta.topic_id vs current_topic。
    pub fn spawn_with_topic(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
        topic_watch: watch::Receiver<fuxi_core::TopicId>,
    ) -> JoinHandle<()> {
        Self::spawn_inner(
            intervener,
            bus,
            fixed_xuannv_watch(xuannv_id),
            trigger_lookup,
            REVIEW_RETRY_BACKOFF_MS,
            Some(topic_watch),
            None,
        )
    }

    /// 块3 测试路径：注入**分身池** watch——每条事件按 `ev.meta.topic_id` 路由到
    /// 归属 topic 的活分身。生产 `spawn` 走同一 `spawn_inner` 路径（pool 模式）；
    /// 这里给单测一个能塞自定义 topic→分身映射的入口。`general_fallback` 是
    /// 无 topic_id / 平台级事件的兜底分身（生产里是 general 分身，镜像 xuannv watch）。
    #[cfg(test)]
    pub(crate) fn spawn_with_pool_for_test(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        general_fallback: AgentId,
        pool_watch: watch::Receiver<std::collections::HashMap<fuxi_core::TopicId, AgentId>>,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        Self::spawn_inner(
            intervener,
            bus,
            fixed_xuannv_watch(general_fallback),
            trigger_lookup,
            REVIEW_RETRY_BACKOFF_MS,
            None,
            Some(pool_watch),
        )
    }

    /// 测试专用：注入自定义 backoff（避免实测拖慢 1.7s）。
    #[cfg(test)]
    pub(crate) fn spawn_with_backoff_for_test(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
        backoff_ms: &'static [u64],
    ) -> JoinHandle<()> {
        Self::spawn_inner(
            intervener,
            bus,
            fixed_xuannv_watch(xuannv_id),
            trigger_lookup,
            backoff_ms,
            None,
            None,
        )
    }

    /// 测试专用：注入活的玄女 id watch，验证 bridge 实时跟随 id 漂移（a01cfab5）。
    #[cfg(test)]
    pub(crate) fn spawn_with_xuannv_watch_for_test(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_watch: watch::Receiver<Option<AgentId>>,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        Self::spawn_inner(
            intervener,
            bus,
            xuannv_watch,
            trigger_lookup,
            REVIEW_RETRY_BACKOFF_MS,
            None,
            None,
        )
    }

    fn spawn_inner(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_watch: watch::Receiver<Option<AgentId>>,
        trigger_lookup: Arc<dyn TriggerLookup>,
        backoff_ms: &'static [u64],
        topic_watch: Option<watch::Receiver<fuxi_core::TopicId>>,
        pool_watch: Option<watch::Receiver<std::collections::HashMap<fuxi_core::TopicId, AgentId>>>,
    ) -> JoinHandle<()> {
        let mut sub = bus.subscribe();
        // bus 既要喂 subscribe（已 move 进上面这行），又要在 handle_event 里用作 publish
        // ReviewRequestTimeout 的句柄——clone 一份给闭包持。
        let bus_for_handler = bus.clone();
        tokio::spawn(async move {
            while let Some(item) = sub.next().await {
                let Ok(ev) = item else {
                    // 即使底层出错也尽量继续；具体 Lagged 已被 subscribe 过滤。
                    continue;
                };
                // 块3：分身池模式优先——每条事件按 `ev.meta.topic_id` 路由到归属
                // topic 的**活分身**作为注入 target，而非单玄女 + current_topic filter。
                // 这是跨 topic 串味的根因修复（357da78a）：milestone 不再透传到「当前
                // 玄女」，而是定向给它归属 topic 的分身——别的 topic 分身零打扰。
                let target_xuannv = if let Some(pool_rx) = pool_watch.as_ref() {
                    // 无 topic_id 的旧事件 / 平台级事件兜底 general（事件没归属 topic）。
                    let ev_topic = ev.meta.topic_id.unwrap_or_else(fuxi_core::TopicId::general);
                    // borrow 要尽早 drop——下面 enqueue 分支有 .await，watch guard 不能跨 await。
                    let active = pool_rx.borrow().get(&ev_topic).copied();
                    match active {
                        Some(id) => Some(id),
                        // 该 topic 无活分身（dormant / 从未起）。general 永远兜底到
                        // xuannv_watch 镜像；非 general 的 dormant topic → 块4 持久队列
                        // 落库「信号不丢」（a01cfab5），分身 respawn 后 drain 补发（块5）。
                        None if ev_topic == fuxi_core::TopicId::general() => *xuannv_watch.borrow(),
                        None => {
                            // 块4：归属 topic 分身 dormant 时，里程碑落持久队列（不误打别
                            // topic 分身造成新串味）。非里程碑 dormant 事件本就不该打扰任何
                            // 分身，silent skip。enqueue 后 continue——本轮不注入。
                            if is_cross_topic_milestone(&ev.kind) {
                                enqueue_dormant_milestone(&*intervener, ev_topic, &ev).await;
                                // 块5：enqueue 后触发该 topic 分身 respawn——分身一起来就
                                // drain 持久队列把刚入队的信号补发，不必等用户下次开该 topic。
                                // 已先 enqueue 再 respawn：spawn 内的 drain 一定看得到这条。
                                // 未注入 spawner（mock/兼容期）→ no-op None，信号留队列等
                                // 用户下次进 topic 时 drain。
                                let _ = intervener.ensure_xuannv_for_topic(ev_topic).await;
                            }
                            continue;
                        }
                    }
                } else {
                    // ── 兼容路径（spawn_with / spawn_with_topic，无 pool）：旧单玄女语义 ──
                    // Phase 1 #6：topic filter——非当前 topic 的普通事件 silent，
                    // milestone（决策 7 阈值）仍透传到单玄女。
                    if let Some(rx) = topic_watch.as_ref()
                        && let Some(ev_topic) = ev.meta.topic_id
                    {
                        let current: fuxi_core::TopicId = *rx.borrow();
                        if ev_topic != current && !is_cross_topic_milestone(&ev.kind) {
                            debug!(
                                ev_topic = %ev_topic,
                                current = %current,
                                kind_tag = ?std::mem::discriminant(&ev.kind),
                                "bridge: 跨 topic 普通事件 silent 跳过"
                            );
                            continue;
                        }
                    }
                    // a01cfab5 修：实时读**当前**玄女 id（跟随 respawn 漂移）。
                    *xuannv_watch.borrow()
                };
                // 无可注入 target（玄女未就绪 None）→ 跳过本事件。
                let Some(xuannv_id) = target_xuannv else {
                    debug!("bridge: 无可注入分身 target（玄女未就绪），跳过事件");
                    continue;
                };
                handle_event(
                    &*intervener,
                    &*trigger_lookup,
                    xuannv_id,
                    &bus_for_handler,
                    backoff_ms,
                    ev,
                )
                .await;
            }
            debug!("SystemEventBridge: 订阅流结束，退出");
        })
    }
}

/// Phase 1 决策 7：跨 topic 但仍应该让当前玄女知道的 milestone EventKind。
/// 其余（AgentResponded / ToolCall* / Thinking* / TaskStateChanged 等普通事件）
/// 默认不跨 topic 透传——避免 topic A 的对话噪音灌进 topic B 的玄女 prompt。
///
/// 决策 7 阈值：`deliverable_produced` / `agent_dead` / `error` / `agent_request_review`。
/// 这里加 [`EventKind::ReviewRequestTimeout`]（review 超时是 review 的失败兜底，
/// 同样关键）。其余按需扩。
fn is_cross_topic_milestone(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::AgentDead { .. }
            | EventKind::AgentRequestReview { .. }
            | EventKind::ReviewRequestTimeout { .. }
            | EventKind::DeliverableProduced { .. }
    )
}

/// 块4：归属 topic 分身 dormant 时，把里程碑事件落持久队列（a01cfab5「信号不丢」）。
///
/// 与活分身路径（[`handle_event`]）**同源**地 build prompt + system_origin——落库的就是
/// 最终文本，块5 分身 respawn 后 drain 直接 `intervene_system(prompt, origin)` 补发，
/// 气泡渲染一致。复用活路径的过滤口径：内部 role（extractor/cangjie）+ idle_ttl 正常
/// 回收的 AgentDead 不入队（噪音，跟活路径 silent 一致）。
///
/// **维护者注意**：本 helper 与 [`handle_event`] 活分身分支共用同一批纯函数
/// `build_request_review_prompt` / `build_death_prompt` / `build_review_timeout_prompt`
/// 算 (prompt, origin)——故活/dormant 两路文本一致。改其中任一 build_* 或 origin 串时
/// 两处自动同步；但**新增** milestone 类型或改过滤口径时要记得**两处都改**，否则
/// dormant 补发会跟活路径不一致（改一处漏另一处的经典坑）。
///
/// 只覆盖会真注入玄女的 3 类 milestone（AgentRequestReview / AgentDead / ReviewRequestTimeout）；
/// DeliverableProduced 虽属 milestone 但活路径从不注入（只当校验凭证），故 dormant 也不入队。
async fn enqueue_dormant_milestone(
    intervener: &dyn Intervener,
    topic: fuxi_core::TopicId,
    ev: &Event,
) {
    let (agent, prompt, origin) = match &ev.kind {
        EventKind::AgentRequestReview {
            agent,
            task,
            deliverable_kind,
            summary,
            artifact_ref,
        } => {
            let role = intervener
                .role_of(*agent)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            if is_internal_role(&role) {
                debug!(%agent, %role, "dormant AgentRequestReview 内部 role，不入队");
                return;
            }
            // 注：artifact 未核实警告（has_deliverable_produced）是活路径的玄女侧产品决策，
            // dormant 落库走常规 review prompt——核实逻辑等分身 respawn 醒来自己判断更稳，
            // 不在补发链路里重复 IO。
            let _ = task;
            let prompt = build_request_review_prompt(
                *agent,
                &role,
                *deliverable_kind,
                summary,
                artifact_ref.as_deref(),
            );
            (*agent, prompt, "review_request")
        }
        EventKind::AgentDead { cause } => {
            let Some(agent) = ev.meta.agent else {
                return;
            };
            // idle_ttl 正常回收不入队（同活路径——纯生命周期信号，玄女无事可做）。
            if cause == "idle_ttl" {
                debug!(%agent, "dormant AgentDead cause=idle_ttl，不入队");
                return;
            }
            let role = intervener
                .role_of(agent)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            if is_internal_role(&role) {
                debug!(%agent, %role, "dormant AgentDead 内部 role，不入队");
                return;
            }
            (agent, build_death_prompt(agent, &role, cause), "agent_dead")
        }
        EventKind::ReviewRequestTimeout {
            agent,
            task,
            waited_for_ms,
            ..
        } => {
            let role = intervener
                .role_of(*agent)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            (
                *agent,
                build_review_timeout_prompt(*agent, &role, *task, *waited_for_ms),
                "review_timeout",
            )
        }
        // DeliverableProduced 等：活路径不注入，dormant 也不入队。
        _ => return,
    };
    if let Err(e) = intervener.enqueue_pending(topic, &prompt, origin).await {
        // 落库失败是「信号不丢」最后一道防线被击穿——必须 warn 可见（不像普通 skip）。
        warn!(error = %e, %agent, %topic, "bridge: dormant 里程碑入持久队列失败，信号可能丢失");
    } else {
        debug!(%agent, %topic, origin, "bridge: dormant 里程碑已落持久队列，待 respawn 补发");
    }
}

/// 事件分派——拆出单函数方便直接单测（不用 spawn 真任务）。
async fn handle_event(
    intervener: &dyn Intervener,
    trigger_lookup: &dyn TriggerLookup,
    xuannv_id: AgentId,
    bus: &EventBus,
    backoff_ms: &[u64],
    ev: Event,
) {
    match ev.kind {
        EventKind::TriggerFired {
            id,
            fired_at,
            cause,
        } => {
            let Some(intent) = trigger_lookup.intent(&id).await else {
                warn!(trigger = id, "TriggerFired 对应的 trigger 已不在库中，跳过");
                return;
            };
            let prompt = build_trigger_prompt(&id, fired_at, &cause, &intent);
            if let Err(e) = intervener
                .intervene_system(xuannv_id, false, &prompt, "trigger_fired")
                .await
            {
                warn!(error = %e, "bridge: intervene(TriggerFired) 失败——玄女可能已下线");
            }
        }
        EventKind::AgentDead { cause } => {
            // 过滤：只处理非玄女门客的死亡——玄女自己死了没法再 intervene，且会造成回响。
            let Some(agent_id) = ev.meta.agent else {
                debug!("AgentDead 缺 meta.agent（平台级死亡），跳过");
                return;
            };
            if agent_id == xuannv_id {
                debug!("AgentDead 目标是玄女自身，跳过——否则就对死人说话");
                return;
            }
            let role = intervener
                .role_of(agent_id)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            // 内部 role（如 extractor）的死亡事件不抄给玄女——是后台自动管理的，
            // 噪音 > 价值。日志里仍记，便于排错。
            if is_internal_role(&role) {
                debug!(%agent_id, %role, "AgentDead 内部 role，跳过抄送");
                return;
            }
            // issue 3871902a(a)：idle_ttl 正常回收下线不注入玄女。idle GC 只回收
            // Idle 门客——按定义它没有在跑的 task，下线纯属生命周期回收，玄女除了
            // 回「不续派」无事可做，是噪音。仅当下线 cause 非 idle_ttl（崩溃 / 异常 /
            // 用户主动 kill 等）才报玄女。注意：仍要走下面的 L2 归档（idle 回收的
            // ephemeral worker 的 worktree 也得归档），所以只跳过通知不 return。
            if cause == "idle_ttl" {
                debug!(%agent_id, %role, "AgentDead cause=idle_ttl 正常回收，跳过注入玄女（仍归档 L2）");
            } else {
                let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
                let interrupt_first = should_interrupt_worker_report(lag_ms);
                info!(
                    %agent_id,
                    %role,
                    lag_ms,
                    interrupt_first,
                    "bridge: 转发门客下线回报到玄女"
                );
                let prompt = build_death_prompt(agent_id, &role, &cause);
                if let Err(e) = intervener
                    .intervene_system(xuannv_id, interrupt_first, &prompt, "agent_dead")
                    .await
                {
                    warn!(error = %e, "bridge: intervene(AgentDead) 失败");
                }
            }
            // Decision 21 phase 3：若该门客住在 L2 ephemeral 工作区 → 自动归档。
            // 反查门客 worktree path → 命中 ephemeral/<task>/ 形态 → 调
            // archive_l2_workspace（reason=TaskCompleted）。worktree_of None 或
            // 路径不匹配 → silent skip（属 L0/L1/L3 / 已不在册门客）。
            if let Some(path) = intervener.worktree_of(agent_id).await
                && let Some((project_id, task_id)) = project_task_from_l2_path(&path)
            {
                info!(
                    %agent_id,
                    %project_id,
                    %task_id,
                    "bridge: AgentDead 触发 L2 ephemeral 自动归档"
                );
                if let Err(e) = intervener
                    .archive_l2_workspace(
                        project_id,
                        task_id,
                        fuxi_core::ArchiveReason::TaskCompleted,
                    )
                    .await
                {
                    warn!(error = %e, "bridge: AgentDead 自动归档 L2 失败");
                }
            }
        }
        EventKind::AgentRequestReview {
            agent,
            task,
            deliverable_kind,
            ref summary,
            ref artifact_ref,
        } => {
            // Decision 13 核心：门客主动 nudge 是占玄女 attention 的唯一通路。
            // 不去重 / 不限频——门客侧自决何时 nudge；桥不替他做产品决策。
            let role = intervener
                .role_of(agent)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            // issue 3871902a(b)：内部 role（cangjie/extractor）的经验抽取产物
            // （research_summary）由抽取管线自动唤起、反复呈递，是高频噪音——
            // 玄女本就不该 review 内部角色产物（路由规则「内部角色不可派活」）。
            // 跟 AgentDead / TaskDone 路径的内部 role 过滤保持一致。
            if is_internal_role(&role) {
                debug!(%agent, %role, "AgentRequestReview 内部 role，跳过注入玄女");
                return;
            }
            let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
            info!(
                %agent,
                %role,
                kind = deliverable_kind_tag(deliverable_kind),
                lag_ms,
                "bridge: 转发 AgentRequestReview 到玄女（带 retry）"
            );
            // issue 1d816926：artifact（apk 等文件）类必须有 DeliverableProduced
            // 凭证。门客 hallucinate「已落 PWA」但没真跑 deliverable produce 时，
            // 事件流里没有这条 → 换「未核实」prompt 让玄女打回，别把幻觉转述给用户。
            // 只卡 artifact——code_change 走 WorkspaceCommitted 校验、研究类无文件，
            // 都不该在这里误拒。
            let prompt = if matches!(deliverable_kind, fuxi_core::DeliverableKind::Artifact)
                && !intervener.has_deliverable_produced(task).await
            {
                warn!(
                    %agent,
                    %task,
                    "bridge: 门客呈递 artifact 但事件流无 DeliverableProduced 凭证 → 注入未核实警告"
                );
                build_unverified_deliverable_prompt(agent, &role, summary, artifact_ref.as_deref())
            } else {
                build_request_review_prompt(
                    agent,
                    &role,
                    deliverable_kind,
                    summary,
                    artifact_ref.as_deref(),
                )
            };
            // Decision 21 phase 1：kind=code_change + artifact_ref 形如 "sha:<hex>"
            // → 推断为 WorkspaceCommitted。从 worktree path 反查 (project, role)
            // 拼 WorkspaceId，命中即发；否则 silent skip。
            // 让 WorkspaceCommitted EventKind 不再是死字段——agent 自报 commit
            // 时事件流自动留痕，玄女 / firehose / IM 可见。
            if matches!(deliverable_kind, fuxi_core::DeliverableKind::CodeChange)
                && let Some(sha) = artifact_ref.as_deref().and_then(|s| s.strip_prefix("sha:"))
                && let Some(path) = intervener.worktree_of(agent).await
                && let Some((workspace_id, branch)) = workspace_id_from_l3_path(&path)
            {
                let mut meta = EventMeta::now();
                meta.agent = ev.meta.agent;
                meta.task = ev.meta.task;
                meta.session = ev.meta.session;
                if let Err(e) = bus.publish(Event {
                    meta,
                    kind: EventKind::WorkspaceCommitted {
                        workspace_id,
                        commit_sha: sha.to_string(),
                        branch,
                    },
                }) {
                    debug!(error = %e, "bridge: publish WorkspaceCommitted 失败 (silent)");
                }
            }
            // AgentRequestReview 永远 interrupt——门客主动找玄女 = 高优先级
            // attention 信号，不让中间 turn 把它挤晚。
            let started = std::time::Instant::now();
            if let Err(e) = try_intervene_with_retry(
                intervener,
                xuannv_id,
                &prompt,
                backoff_ms,
                "review_request",
            )
            .await
            {
                let waited_for_ms = started.elapsed().as_millis() as u64;
                warn!(error = %e, %agent, waited_for_ms, "bridge: AgentRequestReview retry 全失败，发 ReviewRequestTimeout 兜底");
                let mut meta = EventMeta::now();
                meta.agent = ev.meta.agent;
                meta.task = ev.meta.task;
                meta.session = ev.meta.session;
                if let Err(e) = bus.publish(Event {
                    meta,
                    kind: EventKind::ReviewRequestTimeout {
                        original_event_id: ev.meta.id,
                        agent,
                        task,
                        waited_for_ms,
                    },
                }) {
                    warn!(error = %e, "bridge: publish ReviewRequestTimeout 失败");
                }
            }
        }
        EventKind::ReviewRequestTimeout {
            agent,
            task,
            waited_for_ms,
            ..
        } => {
            // 兜底事件：原 AgentRequestReview 玄女漏看了——这条更要 push 进去。
            let role = intervener
                .role_of(agent)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
            let interrupt_first = should_interrupt_worker_report(lag_ms);
            info!(
                %agent,
                %role,
                waited_for_ms,
                lag_ms,
                interrupt_first,
                "bridge: 转发 ReviewRequestTimeout 到玄女"
            );
            let prompt = build_review_timeout_prompt(agent, &role, task, waited_for_ms);
            if let Err(e) = intervener
                .intervene_system(xuannv_id, interrupt_first, &prompt, "review_timeout")
                .await
            {
                warn!(error = %e, "bridge: intervene(ReviewRequestTimeout) 失败");
            }
        }
        EventKind::OrchestratorCcReceived {
            from_user_to, text, ..
        } => {
            // 过滤条件：抄送的目标（meta.agent）必须就是玄女；否则这不是发给她的抄送。
            let Some(target) = ev.meta.agent else {
                return;
            };
            if target != xuannv_id {
                debug!("OrchestratorCcReceived meta.agent != xuannv，跳过");
                return;
            }
            // 安全网：若某种代码路径把 xuannv 自己塞进 from_user_to（不该发生）就跳过，
            // 避免"玄女抄送给玄女"的回响。
            if from_user_to == xuannv_id {
                return;
            }
            let role = intervener
                .role_of(from_user_to)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            let prompt = build_cc_prompt(from_user_to, &role, &text);
            if let Err(e) = intervener
                .intervene_system(xuannv_id, false, &prompt, "carbon_copy")
                .await
            {
                warn!(error = %e, "bridge: intervene(OrchestratorCcReceived) 失败");
            }
        }
        EventKind::TaskStateChanged { to, .. } => {
            // Bug 修：task 终态时归档关联 L2 ephemeral 工作区。AgentDead 路径
            // （bridge.rs:531-554）只在门客死亡时生效——但实测门客被 idle GC
            // 走 / 状态机 bug 卡 ShuttingDown 不死时，task 已 done 但 workspace
            // 永远不归档。这里加第二条触发器作为兜底。archive 是幂等的
            // （ephemeral_workspace.rs:201 不存在 → silent Ok），跟 AgentDead
            // 路径冲突时谁先到谁干活，另一边 noop。
            if !matches!(
                to,
                fuxi_core::task::TaskState::Done | fuxi_core::task::TaskState::Cancelled
            ) {
                return;
            }
            let Some(task_id) = ev.meta.task else {
                return;
            };
            if let Err(e) = intervener
                .archive_l2_for_task(task_id, fuxi_core::ArchiveReason::TaskCompleted)
                .await
            {
                warn!(error = %e, "bridge: TaskStateChanged 自动归档 L2 失败");
            }
            // issue a58e45b4 兜底：门客 task 完成时，如果他没主动跑 sentinel，
            // 玄女不会收到 AgentRequestReview → 用户必须手敲「门客完成了」。
            // bridge 在终态时给玄女注入一条 [TASK_DONE] 系统消息——不依赖门客自觉。
            //
            // 跳过条件：
            //   1. 没 worker agent_id（平台级 task，无对接对象）
            //   2. worker == xuannv（玄女自己 turn done，对她报告等于自言自语）
            //   3. worker role 是 internal（extractor 等后台 role 的噪音不抄给玄女）
            //
            // 跟 AgentRequestReview 路径并存可能造成同 task 两条玄女消息——但
            // origin 不同（review_request vs task_done）+ 内容不同，redundancy 比
            // 漏看好。dedupe 需要存 task_id 状态，复杂度暂不付。
            if matches!(to, fuxi_core::task::TaskState::Done)
                && let Some(worker_id) = ev.meta.agent
                && worker_id != xuannv_id
            {
                let role = intervener
                    .role_of(worker_id)
                    .await
                    .unwrap_or_else(|| "unknown".to_string());
                if !is_internal_role(&role) {
                    let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
                    let interrupt_first = should_interrupt_worker_report(lag_ms);
                    let prompt = build_task_done_prompt(worker_id, &role, task_id);
                    info!(
                        %worker_id,
                        %role,
                        %task_id,
                        lag_ms,
                        interrupt_first,
                        "bridge: 门客 task done 兜底注入玄女"
                    );
                    if let Err(e) = intervener
                        .intervene_system(xuannv_id, interrupt_first, &prompt, "task_done")
                        .await
                    {
                        warn!(error = %e, "bridge: intervene(TaskDone) 失败");
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use fuxi_core::event::EventMeta;
    use fuxi_events::EventBus;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    #[test]
    fn parse_bool_token_recognizes_common_truthy_values() {
        assert!(parse_bool_token("1"));
        assert!(parse_bool_token("true"));
        assert!(parse_bool_token("YES"));
        assert!(parse_bool_token(" on "));
        assert!(!parse_bool_token("0"));
        assert!(!parse_bool_token("false"));
        assert!(!parse_bool_token("no"));
        assert!(!parse_bool_token(""));
    }

    #[test]
    fn bridge_delivery_lag_ms_never_negative() {
        let future = Utc::now() + chrono::Duration::seconds(1);
        assert_eq!(bridge_delivery_lag_ms(future), 0);
    }

    // ─── mock intervener ─────────────────────────────────────

    #[derive(Default)]
    struct MockIntervener {
        calls: Mutex<Vec<(AgentId, bool, String)>>,
        roles: Mutex<HashMap<AgentId, String>>,
        /// 前 N 次 intervene 返 Err，第 N+1 次起返 Ok。
        /// `None` = 全 Ok（默认）；`Some(usize::MAX)` = 永远失败。
        fail_first_n: Mutex<Option<usize>>,
        worktrees: Mutex<HashMap<AgentId, std::path::PathBuf>>,
        archive_calls: Mutex<Vec<(fuxi_core::ProjectId, TaskId, fuxi_core::ArchiveReason)>>,
        /// 标记哪些 task 有真 DeliverableProduced 凭证（issue 1d816926 校验测试用）。
        deliverable_tasks: Mutex<std::collections::HashSet<TaskId>>,
        /// 块4：记录 enqueue_pending 调用（topic, prompt, origin）——dormant 落库测试用。
        enqueue_calls: Mutex<Vec<(fuxi_core::TopicId, String, String)>>,
        /// 块5：记录 ensure_xuannv_for_topic 调用（respawn 触发）——dormant 补发回归用。
        respawn_calls: Mutex<Vec<fuxi_core::TopicId>>,
    }

    impl MockIntervener {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        async fn set_role(&self, id: AgentId, role: &str) {
            self.roles.lock().await.insert(id, role.to_string());
        }

        /// 让前 N 次 intervene 失败（用于 retry 测试）。
        async fn set_fail_first_n(&self, n: usize) {
            *self.fail_first_n.lock().await = Some(n);
        }

        /// 让所有 intervene 失败（用于 timeout 兜底测试）。
        async fn set_fail_always(&self) {
            *self.fail_first_n.lock().await = Some(usize::MAX);
        }

        async fn snapshot(&self) -> Vec<(AgentId, bool, String)> {
            self.calls.lock().await.clone()
        }

        async fn set_worktree(&self, id: AgentId, path: std::path::PathBuf) {
            self.worktrees.lock().await.insert(id, path);
        }

        /// 标记某 task 有真 DeliverableProduced 凭证。
        async fn set_deliverable_produced(&self, task: TaskId) {
            self.deliverable_tasks.lock().await.insert(task);
        }

        async fn archive_snapshot(
            &self,
        ) -> Vec<(fuxi_core::ProjectId, TaskId, fuxi_core::ArchiveReason)> {
            self.archive_calls.lock().await.clone()
        }

        async fn enqueue_snapshot(&self) -> Vec<(fuxi_core::TopicId, String, String)> {
            self.enqueue_calls.lock().await.clone()
        }

        async fn respawn_snapshot(&self) -> Vec<fuxi_core::TopicId> {
            self.respawn_calls.lock().await.clone()
        }
    }

    #[async_trait]
    impl Intervener for MockIntervener {
        async fn intervene(
            &self,
            agent_id: AgentId,
            interrupt_first: bool,
            text: &str,
        ) -> Result<()> {
            // 先记录调用次数（无论成功失败都要数到，retry 才能验证次数）。
            let call_idx = {
                let mut calls = self.calls.lock().await;
                calls.push((agent_id, interrupt_first, text.to_string()));
                calls.len() - 1
            };
            let mut fail_lock = self.fail_first_n.lock().await;
            if let Some(n) = *fail_lock {
                if n == usize::MAX || call_idx < n {
                    return Err(crate::error::OrchestratorError::Other(format!(
                        "mock fail (call_idx={call_idx}, fail_first_n={n})"
                    )));
                }
                // 已耗尽 fail 配额，consume 之后此 mock 之后都 Ok。
                *fail_lock = None;
            }
            Ok(())
        }
        async fn role_of(&self, agent_id: AgentId) -> Option<String> {
            self.roles.lock().await.get(&agent_id).cloned()
        }
        async fn worktree_of(&self, agent_id: AgentId) -> Option<std::path::PathBuf> {
            self.worktrees.lock().await.get(&agent_id).cloned()
        }
        async fn archive_l2_workspace(
            &self,
            project_id: fuxi_core::ProjectId,
            task: TaskId,
            reason: fuxi_core::ArchiveReason,
        ) -> Result<()> {
            self.archive_calls
                .lock()
                .await
                .push((project_id, task, reason));
            Ok(())
        }
        async fn archive_l2_for_task(
            &self,
            task: TaskId,
            reason: fuxi_core::ArchiveReason,
        ) -> Result<()> {
            // mock：把 task-centric 调用记成 project_id="by-task" 占位，
            // 测试断言自己 unwrap reason / task 即可。生产 Fuxi 覆盖时走真路径。
            let placeholder =
                fuxi_core::ProjectId::new("by-task".to_string()).expect("placeholder project id");
            self.archive_calls
                .lock()
                .await
                .push((placeholder, task, reason));
            Ok(())
        }
        async fn has_deliverable_produced(&self, task: TaskId) -> bool {
            self.deliverable_tasks.lock().await.contains(&task)
        }
        async fn enqueue_pending(
            &self,
            topic_id: fuxi_core::TopicId,
            prompt: &str,
            system_origin: &str,
        ) -> Result<()> {
            self.enqueue_calls.lock().await.push((
                topic_id,
                prompt.to_string(),
                system_origin.to_string(),
            ));
            Ok(())
        }
        async fn ensure_xuannv_for_topic(&self, topic: fuxi_core::TopicId) -> Option<AgentId> {
            self.respawn_calls.lock().await.push(topic);
            None
        }
    }

    // ─── mock trigger lookup ─────────────────────────────────

    struct MockLookup(HashMap<String, String>);

    #[async_trait]
    impl TriggerLookup for MockLookup {
        async fn intent(&self, id: &str) -> Option<String> {
            self.0.get(id).cloned()
        }
    }

    fn empty_lookup() -> Arc<dyn TriggerLookup> {
        Arc::new(MockLookup(HashMap::new()))
    }

    // ─── 小工具：等桥消化完一批事件 ───────────────────────────

    async fn wait_call(mock: &Arc<MockIntervener>, at_least: usize) {
        for _ in 0..100 {
            if mock.snapshot().await.len() >= at_least {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("等 intervene 被调至少 {at_least} 次超时");
    }

    // ─── tests ────────────────────────────────────────────────

    /// 门客 AgentDead → 桥 intervene 玄女一次，prompt 含 role + cause。
    #[tokio::test]
    async fn agent_dead_of_worker_wakes_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "codex-coder").await;

        let _handle =
            SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        // 等桥完成 subscribe（broadcast 漏发给未订阅者）——发一条 ping 再推真事件
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "cc stream closed".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "应且仅 intervene 一次");
        let (target, interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(*interrupt_first, "默认应打断玄女以优先投递门客回报");
        assert!(text.contains("下线"), "prompt 含下线: {text}");
        assert!(text.contains("codex-coder"), "prompt 含 role: {text}");
        assert!(text.contains("cc stream closed"), "prompt 含 cause: {text}");
    }

    /// Decision 21 phase 3：门客死且 worktree 在 L2 ephemeral 路径下 → 桥
    /// 自动调 archive_l2_workspace（reason=TaskCompleted），无需玄女手动介入。
    #[tokio::test]
    async fn agent_dead_in_l2_ephemeral_auto_archives() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let task = TaskId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        // 假造 L2 ephemeral 路径——文件不必真实存在；project_task_from_l2_path
        // 走 ancestors() 字符串匹配（canonicalize 失败时 fallback to_path_buf）。
        let dir = tempfile::tempdir().expect("tmpdir");
        let l2_path = dir
            .path()
            .join("erp")
            .join("ephemeral")
            .join(task.to_string());
        std::fs::create_dir_all(&l2_path).expect("mkdir l2");
        mock.set_worktree(worker, l2_path).await;

        let _handle =
            SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "task done".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        // 给 archive 调用一点时间——它在 intervene 之后跑。
        for _ in 0..100 {
            if !mock.archive_snapshot().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let archives = mock.archive_snapshot().await;
        assert_eq!(archives.len(), 1, "应触发一次 L2 archive");
        let (project_id, archived_task, reason) = &archives[0];
        assert_eq!(project_id.as_str(), "erp");
        assert_eq!(*archived_task, task);
        assert_eq!(*reason, fuxi_core::ArchiveReason::TaskCompleted);
    }

    /// 门客在 L3 sandbox / 普通 worktree 死亡 → 不应误归档（L2 hook 严格匹配 ephemeral/）。
    #[tokio::test]
    async fn agent_dead_in_l3_sandbox_does_not_archive_l2() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        let dir = tempfile::tempdir().expect("tmpdir");
        let l3_path = dir.path().join("erp").join("sandboxes").join("luban");
        std::fs::create_dir_all(&l3_path).expect("mkdir l3");
        mock.set_worktree(worker, l3_path).await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "shutdown".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.archive_snapshot().await.is_empty(),
            "L3 sandbox 死不应触发 L2 archive"
        );
    }

    /// Bug 修：task 状态变 Done → 触发 L2 归档兜底（AgentDead 路径漏的场景）。
    #[tokio::test]
    async fn task_state_done_triggers_l2_archive() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let task = TaskId::new();

        let mock = MockIntervener::new();
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Delivering,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        for _ in 0..100 {
            if !mock.archive_snapshot().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let archives = mock.archive_snapshot().await;
        assert_eq!(
            archives.len(),
            1,
            "TaskStateChanged Done 应触发一次 L2 archive"
        );
        let (_project, archived_task, reason) = &archives[0];
        assert_eq!(*archived_task, task);
        assert_eq!(*reason, fuxi_core::ArchiveReason::TaskCompleted);
    }

    /// task Cancelled 终态同样触发 L2 归档。
    #[tokio::test]
    async fn task_state_cancelled_triggers_l2_archive() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let task = TaskId::new();

        let mock = MockIntervener::new();
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Blocked,
                to: fuxi_core::task::TaskState::Cancelled,
            },
        })
        .expect("publish");

        for _ in 0..100 {
            if !mock.archive_snapshot().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(mock.archive_snapshot().await.len(), 1);
    }

    /// 非终态（Ready / InProgress / Delivering）不应触发归档——避免反复刷盘 + 错误事件。
    #[tokio::test]
    async fn task_state_non_terminal_does_not_archive() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let task = TaskId::new();

        let mock = MockIntervener::new();
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::New,
                to: fuxi_core::task::TaskState::InProgress,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.archive_snapshot().await.is_empty(),
            "非终态不应触发 archive"
        );
    }

    /// 玄女自己 AgentDead → 桥不 intervene（否则就是对死人说话 + 回响）。
    #[tokio::test]
    async fn agent_dead_of_xuannv_is_ignored() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();

        let mock = MockIntervener::new();
        let _handle =
            SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(xuannv);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "fuxi shutdown".into(),
            },
        })
        .expect("publish");

        // 给桥 50ms 处理；应无 intervene。
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "玄女自己死不应触发 intervene"
        );
    }

    /// TriggerFired → 桥查 TriggerLookup 拿 intent → 拼三段式 prompt → intervene。
    #[tokio::test]
    async fn trigger_fired_wakes_xuannv_with_three_stage_prompt() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();

        let mock = MockIntervener::new();
        let mut map = HashMap::new();
        map.insert("trg_abc".to_string(), "每周五 9 点 review PR".to_string());
        let lookup: Arc<dyn TriggerLookup> = Arc::new(MockLookup(map));

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, lookup);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let at = Utc::now();
        bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFired {
                id: "trg_abc".into(),
                fired_at: at,
                cause: "scheduled".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1);
        let (target, interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(!*interrupt_first);
        assert!(
            text.starts_with("[TRIGGER_FIRED id=trg_abc "),
            "head: {text}"
        );
        assert!(text.contains("cause=scheduled"), "cause: {text}");
        assert!(text.contains("每周五 9 点 review PR"), "intent: {text}");
        assert!(text.contains("[INSTRUCTION:"), "instruction: {text}");
    }

    /// TriggerFired 但 TriggerLookup 找不到 intent → 跳过，不 intervene。
    #[tokio::test]
    async fn trigger_fired_without_intent_is_skipped() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let mock = MockIntervener::new();

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFired {
                id: "trg_missing".into(),
                fired_at: Utc::now(),
                cause: "manual".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(mock.snapshot().await.is_empty(), "找不到 intent 应静默跳过");
    }

    /// OrchestratorCcReceived 目标=玄女 → 桥必 intervene 玄女一次（公理 #1 显式沟通）。
    #[tokio::test]
    async fn orchestrator_cc_received_wakes_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(xuannv);
        bus.publish(Event {
            meta,
            kind: EventKind::OrchestratorCcReceived {
                from_user_to: worker,
                text: "你好门客".into(),
                original_intervention_id: uuid::Uuid::new_v4(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "抄送应 intervene 玄女一次");
        let (target, interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(!*interrupt_first, "抄送是追加式，不打断当前 turn");
        assert!(
            text.contains("[CC · 仅知情]"),
            "prompt 应含 [CC · 仅知情] 标识: {text}"
        );
        assert!(text.contains("luban"), "prompt 应含 role: {text}");
        assert!(text.contains("你好门客"), "prompt 应含原文: {text}");
    }

    /// 抄送目标 meta.agent 不是玄女 → 跳过（避免误触）。
    #[tokio::test]
    async fn orchestrator_cc_received_for_other_agent_is_ignored() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let other = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(other); // 抄送对象不是玄女
        bus.publish(Event {
            meta,
            kind: EventKind::OrchestratorCcReceived {
                from_user_to: worker,
                text: "别人的抄送".into(),
                original_intervention_id: uuid::Uuid::new_v4(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "非玄女的抄送不应触发 intervene"
        );
    }

    /// broadcast Lagged 不打断桥——丢几条后续事件仍要被处理。
    #[tokio::test]
    async fn bridge_survives_broadcast_lag() {
        // 极小 buffer 迫使未消费 receiver 触发 Lagged。
        let store = fuxi_events::EventStore::connect_memory()
            .await
            .expect("store");
        let cfg = fuxi_events::EventBusConfig {
            buffer: 2,
            writer_queue: 4096,
            lag_threshold: 10000,
        };
        let bus = EventBus::new(store, cfg);
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "painter").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 先狂推 200 条桥不关心的事件（UserPrompted）——buffer=2 保证桥被 Lag。
        for i in 0..200 {
            bus.publish(Event {
                meta: EventMeta::now(),
                kind: EventKind::UserPrompted {
                    text: format!("noise-{i}"),
                },
            })
            .expect("publish noise");
        }

        // 再给一条真 AgentDead——Lag 后桥仍应响应。
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "late but real".into(),
            },
        })
        .expect("publish dead");

        // 必须能在短时间内拿到这条 intervene。
        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.iter().any(|(_, _, t)| t.contains("late but real")),
            "Lag 后桥仍应处理 AgentDead: {calls:?}"
        );
    }

    // 历史 `task_done_no_longer_copies_to_xuannv` 测试在 issue a58e45b4 修后
    // 被反转——见 `bridge_intervenes_xuannv_on_worker_task_done` 等 4 条新测试。
    // Decision 13 的"中间过程 silent"精神不变，但 TaskDone 终态是用户级
    // 决策点，从"silent"调成"兜底 nudge"——门客**应**主动跑 sentinel 仍是首选，
    // 没跑时玄女不再失声。详见 docs/decisions/13-deliverable-boundary-handoff.md
    // 末尾的 v2 修正段。

    /// 中间事件（AgentResponded / ToolCallStarted / ToolCallFinished）默认 silent
    /// —— attention filter 白名单生效（Decision 13）。
    #[tokio::test]
    async fn bridge_silent_on_middle_event() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut m1 = EventMeta::now();
        m1.agent = Some(worker);
        bus.publish(Event {
            meta: m1,
            kind: EventKind::AgentResponded {
                text: "中间产物 1".into(),
                artifact_ref: None,
            },
        })
        .expect("publish AgentResponded");

        let mut m2 = EventMeta::now();
        m2.agent = Some(worker);
        bus.publish(Event {
            meta: m2,
            kind: EventKind::ToolCallStarted {
                tool: "Read".into(),
                args: serde_json::json!({}),
            },
        })
        .expect("publish ToolCallStarted");

        let mut m3 = EventMeta::now();
        m3.agent = Some(worker);
        bus.publish(Event {
            meta: m3,
            kind: EventKind::ToolCallFinished {
                tool: "Read".into(),
                ok: true,
                output_preview: "ok".into(),
            },
        })
        .expect("publish ToolCallFinished");

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "中间事件不能触发 intervene（Decision 13 attention filter）"
        );
    }

    /// 门客发 AgentRequestReview → 桥触发 intervene 玄女一次，
    /// prompt 含 deliverable_kind + summary + role + artifact_ref。
    #[tokio::test]
    async fn bridge_triggers_intervene_on_request_review() {
        use fuxi_core::DeliverableKind;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = fuxi_core::id::TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: DeliverableKind::CodeChange,
                summary: "重构 dispatch pump 完工，待审".into(),
                artifact_ref: Some("commit:abc1234".into()),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "AgentRequestReview 应触发 intervene 一次");
        let (target, _interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(
            text.contains("code_change"),
            "prompt 含 deliverable_kind: {text}"
        );
        assert!(
            text.contains("重构 dispatch pump 完工，待审"),
            "prompt 含 summary: {text}"
        );
        assert!(text.contains("luban"), "prompt 含 role: {text}");
        assert!(
            text.contains("commit:abc1234"),
            "prompt 含 artifact_ref: {text}"
        );
    }

    /// issue 1d816926 回归：门客呈递 artifact 但事件流无 DeliverableProduced 凭证
    /// → 玄女收到「未核实」警告 prompt，而非按已交付的正常 REVIEW_REQUEST。
    #[tokio::test]
    async fn artifact_without_deliverable_produced_gets_unverified_warning() {
        use fuxi_core::DeliverableKind;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        // 注意：不 set_deliverable_produced → 模拟门客没真跑 deliverable produce。

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = fuxi_core::id::TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: DeliverableKind::Artifact,
                summary: "apk 已落 PWA，manifest 在 .../task-x/manifest.json".into(),
                artifact_ref: Some("apk:9.4MB".into()),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "应 intervene 一次（未核实警告）");
        let (target, _i, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(text.contains("未核实"), "应是未核实警告: {text}");
        assert!(
            text.contains("没有") && text.contains("DeliverableProduced"),
            "应说明事件流无凭证: {text}"
        );
    }

    /// issue 1d816926 回归（正路）：artifact 有真 DeliverableProduced 凭证时
    /// 走正常 REVIEW_REQUEST，不误报未核实。
    #[tokio::test]
    async fn artifact_with_deliverable_produced_normal_review() {
        use fuxi_core::DeliverableKind;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let task = fuxi_core::id::TaskId::new();
        mock.set_deliverable_produced(task).await; // 门客真交付了

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: DeliverableKind::Artifact,
                summary: "apk 已交付".into(),
                artifact_ref: Some("apk:9.4MB".into()),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        let (_t, _i, text) = &calls[0];
        assert!(!text.contains("未核实"), "有凭证不应报未核实: {text}");
        assert!(
            text.contains("[REVIEW_REQUEST]"),
            "应是正常审阅 prompt: {text}"
        );
    }

    /// issue 3871902a(b) 回归：内部 role（cangjie/extractor）的 AgentRequestReview
    /// 不该注入玄女——内部经验抽取产物（research_summary）反复呈递是高频噪音，
    /// 玄女本就不该 review 内部角色产物（见路由规则「内部角色不可派活」）。
    #[tokio::test]
    async fn agent_request_review_from_internal_role_is_ignored() {
        use fuxi_core::DeliverableKind;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "cangjie").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = fuxi_core::id::TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: DeliverableKind::ResearchSummary,
                summary: "[{\"score\":0.7,\"reason\":\"...\"}]".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "内部 role 的 AgentRequestReview 不应注入玄女"
        );
    }

    /// issue 3871902a(a) 回归：idle_ttl 正常回收下线不该注入玄女。idle GC 只回收
    /// 处于 Idle 的门客——按定义它没有在跑的 task，下线纯属生命周期回收，玄女
    /// 除了回「不续派」无事可做，是高频噪音。仅当下线 cause 非 idle_ttl（崩溃 /
    /// 异常）才报玄女。
    #[tokio::test]
    async fn agent_dead_idle_ttl_is_suppressed() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "idle_ttl".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "idle_ttl 正常回收下线不应注入玄女"
        );
    }

    /// a01cfab5 回归：玄女 id 在会话中 respawn 漂移后，门客 AgentRequestReview 必须
    /// 注入【当前】玄女 id，不能打到启动期 snapshot 的旧（已死）id——否则
    /// AgentNotFound → retry 耗尽 → 完工信号丢，玄女永远不知道门客干完了。
    #[tokio::test]
    async fn bridge_review_follows_xuannv_id_drift() {
        use fuxi_core::DeliverableKind;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let old_xuannv = AgentId::new();
        let new_xuannv = AgentId::new();
        let worker = AgentId::new();
        let (tx, rx) = watch::channel(Some(old_xuannv));

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        let _h = SystemEventBridge::spawn_with_xuannv_watch_for_test(
            mock.clone(),
            bus.clone(),
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 玄女 respawn → id 漂到 new（bridge 不重启，靠 watch 跟随）。
        tx.send_replace(Some(new_xuannv));

        let task = fuxi_core::id::TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: DeliverableKind::CodeChange,
                summary: "干完了，待审".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "AgentRequestReview 应触发 intervene 一次");
        assert_eq!(
            calls[0].0, new_xuannv,
            "review 必须注入【当前】玄女 id，不是启动期 snapshot 的旧 id"
        );
    }

    /// ReviewRequestTimeout（兜底）→ 桥触发 intervene 玄女，告知她漏了一次审阅。
    #[tokio::test]
    async fn bridge_triggers_intervene_on_review_timeout() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = fuxi_core::id::TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::ReviewRequestTimeout {
                original_event_id: uuid::Uuid::new_v4(),
                agent: worker,
                task,
                waited_for_ms: 1700,
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "ReviewRequestTimeout 应触发 intervene 一次");
        let (target, _, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(
            text.contains("超时") || text.contains("timeout"),
            "prompt 提及超时: {text}"
        );
        assert!(text.contains("luban"), "prompt 含 role: {text}");
    }

    /// 内部 role（extractor）AgentDead 也**不**触发——后台自管理生命周期。
    #[tokio::test]
    async fn extractor_agent_dead_is_not_copied_to_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let extractor = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(extractor, "extractor").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(extractor);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "fuxi shutdown".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "extractor 死不应抄送给玄女"
        );
    }

    // ─── #4 retry + timeout 兜底 ──────────────────────────────

    /// retry 第一次失败、第二次成功 → 共两次调用，最终 Ok。
    /// 直接戳 `try_intervene_with_retry`，不走 bus，避免 bus 异步噪音。
    #[tokio::test]
    async fn review_intervene_retry_succeeds_on_second_try() {
        let xuannv = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_fail_first_n(1).await;

        // 极短 backoff（1+2ms）让测试不拖慢 CI——生产用 REVIEW_RETRY_BACKOFF_MS。
        let result =
            try_intervene_with_retry(&*mock, xuannv, "test prompt", &[1, 2, 4], "review_request")
                .await;
        assert!(result.is_ok(), "第二次应成功: {result:?}");
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 2, "共两次调用：第一次 fail + 第二次 ok");
        assert!(calls.iter().all(|(t, _, _)| *t == xuannv));
    }

    /// retry 全失败 → 返 Err（兜底事件由 handle_event 决定 publish）。
    #[tokio::test]
    async fn review_intervene_retry_returns_err_when_all_fail() {
        let xuannv = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_fail_always().await;

        let result =
            try_intervene_with_retry(&*mock, xuannv, "test prompt", &[1, 2, 4], "review_request")
                .await;
        assert!(result.is_err(), "全失败应返 Err");
        let calls = mock.snapshot().await;
        // backoff len = 3 → 共 1 (首发) + 3 (retry) = 4 次调用。
        assert_eq!(calls.len(), 4, "首发 1 次 + retry 3 次 = 4 次：{calls:?}");
    }

    /// retry 全失败时 bridge publish ReviewRequestTimeout 兜底事件（核心断言）。
    /// 用 spawn_with 走完整流：mock 全 fail → 桥 retry exhaust → bus 上能收到 ReviewRequestTimeout。
    #[tokio::test]
    async fn review_intervene_timeout_publishes_fallback_event() {
        // 极小 buffer 不影响——我们只发 2 条，buffer=64 默认就够。
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        mock.set_fail_always().await;

        // 在 bus 上单独订一个 sub 监听后续事件——必须先订阅再发，broadcast 漏发给晚到者。
        let mut observer = bus.subscribe();

        // 用极短 backoff 避免 1.7s 实测耗时——通过 cfg(test) 隐藏接口。
        let _h = SystemEventBridge::spawn_with_backoff_for_test(
            mock.clone(),
            bus.clone(),
            xuannv,
            empty_lookup(),
            &[1, 2, 4],
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = fuxi_core::id::TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        let original_event_id = meta.id;
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: DeliverableKind::CodeChange,
                summary: "test fallback".into(),
                artifact_ref: None,
            },
        })
        .expect("publish AgentRequestReview");

        // 期间桥跑：4 次 intervene 全 fail → publish ReviewRequestTimeout。
        // 在 observer 上读到的事件流：第 1 条 = 我们刚 publish 的 AgentRequestReview；
        // 后续应能等到一条 ReviewRequestTimeout。给 500ms 总预算（backoff sum ~7ms + 调度噪音）。
        let mut saw_timeout = false;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, observer.next()).await {
                Ok(Some(Ok(ev))) => {
                    if let EventKind::ReviewRequestTimeout {
                        original_event_id: oid,
                        agent,
                        task: t,
                        waited_for_ms,
                    } = &ev.kind
                    {
                        assert_eq!(*oid, original_event_id, "兜底事件须关联原 event id");
                        assert_eq!(*agent, worker);
                        assert_eq!(*t, task);
                        assert!(*waited_for_ms > 0, "waited_for_ms > 0: {waited_for_ms}");
                        saw_timeout = true;
                        break;
                    }
                }
                Ok(Some(Err(_))) => continue, // Lagged 或别的 recv 错——继续
                Ok(None) => break,            // 流结束
                Err(_) => break,              // 超时
            }
        }
        assert!(saw_timeout, "retry 全 fail 应 publish ReviewRequestTimeout");

        // 也验证：retry 的 4 次 intervene 都数到了——
        // AgentRequestReview 首发 1 次 + retry 3 次 = 4 次 REVIEW_REQUEST。
        // ReviewRequestTimeout 自身又触发 1 次 REVIEW_TIMEOUT intervene（白名单）——
        // 共 5 次。这是预期：兜底事件本就要让玄女知情。
        let calls = mock.snapshot().await;
        let review_req_calls = calls
            .iter()
            .filter(|(_, _, t)| t.contains("[REVIEW_REQUEST]"))
            .count();
        assert_eq!(
            review_req_calls, 4,
            "AgentRequestReview 首发 1 + retry 3 = 4 次：{calls:?}"
        );
        let review_timeout_calls = calls
            .iter()
            .filter(|(_, _, t)| t.contains("[REVIEW_TIMEOUT]"))
            .count();
        assert!(
            review_timeout_calls >= 1,
            "兜底 ReviewRequestTimeout 也应触发一次 intervene：{calls:?}"
        );
    }

    /// retry 序列被 const 暴露给 delta 的 e2e fixture 用，验证字面量。
    #[test]
    fn review_retry_backoff_const_matches_decision() {
        assert_eq!(REVIEW_RETRY_BACKOFF_MS, &[200u64, 500, 1000]);
        let total: u64 = REVIEW_RETRY_BACKOFF_MS.iter().sum();
        assert_eq!(total, 1700, "delta 用此 sum 算 timeout 预算（2.5s 留余量）");
    }

    // ─── issue a58e45b4 · TaskStateChanged Done 兜底注入玄女 ───
    // 门客做完没主动跑 sentinel 时，bridge 必须把 TaskStateChanged{Done} 翻成
    // [TASK_DONE] 系统消息塞玄女对话流——否则她蒙在鼓里得用户主动喊。

    #[tokio::test]
    async fn bridge_intervenes_xuannv_on_worker_task_done() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish TaskStateChanged");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = mock.snapshot().await;
        let task_done = calls
            .iter()
            .filter(|(_, _, t)| t.contains("[TASK_DONE]"))
            .collect::<Vec<_>>();
        assert_eq!(
            task_done.len(),
            1,
            "门客 task done 应触发 1 次 [TASK_DONE] 注入：{calls:?}"
        );
        let (target, _interrupt, text) = task_done[0];
        assert_eq!(*target, xuannv, "intervene 目标必须是玄女");
        assert!(text.contains(&worker.to_string()), "提示应含 worker id");
        assert!(text.contains("luban"), "提示应含 role");
        assert!(text.contains(&task.to_string()), "提示应含 task id");
    }

    #[tokio::test]
    async fn bridge_skips_xuannv_self_task_done() {
        // 玄女自己 turn done 时不要再对她注入——会自言自语
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(xuannv, "xuannv").await;
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(xuannv);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.iter().all(|(_, _, t)| !t.contains("[TASK_DONE]")),
            "玄女自己 task done 不应触发 [TASK_DONE]：{calls:?}"
        );
    }

    #[tokio::test]
    async fn bridge_skips_internal_role_task_done() {
        // extractor 等内部 role 完成 task 不抄给玄女（噪音 > 价值）
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let extractor = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(extractor, "extractor").await;
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(extractor);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.iter().all(|(_, _, t)| !t.contains("[TASK_DONE]")),
            "extractor task done 不应触发 [TASK_DONE]：{calls:?}"
        );
    }

    #[tokio::test]
    async fn bridge_skips_cangjie_role_task_done() {
        // issue eebe38ef：cangjie 是 insight extractor 短任务，每个 task done /
        // batch judge 都派一只——若不静默，玄女被高频 "[TASK_DONE] role=cangjie"
        // 占 attention，与 extractor 同 noise pattern。
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let cangjie = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(cangjie, "cangjie").await;
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(cangjie);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.iter().all(|(_, _, t)| !t.contains("[TASK_DONE]")),
            "cangjie task done 不应触发 [TASK_DONE]：{calls:?}"
        );
    }

    #[tokio::test]
    async fn bridge_does_not_intervene_on_non_done_task_state() {
        // Cancelled 不触发——只有 Done 算"完成可 review"。Cancelled 的归档逻辑
        // 已存在路径，但 nudge 玄女对取消的 task 没意义。
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Cancelled,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.iter().all(|(_, _, t)| !t.contains("[TASK_DONE]")),
            "Cancelled 不应触发 [TASK_DONE]：{calls:?}"
        );
    }

    // ─── Phase 1 #6 · topic filter ────────────────────────────

    /// 跨 topic 的普通 worker event（譬如 AgentResponded）默认 silent——不进入
    /// 当前玄女 prompt。这是 Phase 1 治"多话题打断"污染的核心断言。
    #[tokio::test]
    async fn bridge_filters_cross_topic_non_milestone_events() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let current_topic = TopicId::new();
        let other_topic = TopicId::new();
        let (tx, rx) = watch::channel(current_topic);
        let _h = SystemEventBridge::spawn_with_topic(
            mock.clone(),
            bus.clone(),
            xuannv,
            empty_lookup(),
            rx,
        );

        // 跨 topic 的普通 worker event：other_topic 的 AgentResponded
        // —— bridge 不直接处理 AgentResponded，所以即使没 filter 它也不会
        // intervene。但我们要验证的是：跨 topic 时 TaskStateChanged → Done 也
        // 被过滤掉（这条本来会触发 [TASK_DONE] 注入）。
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        meta.topic_id = Some(other_topic);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Delivering,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(60)).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.is_empty(),
            "跨 topic 普通事件应被 silent 跳过：{calls:?}"
        );

        // 控制实验：把 current 切到 other_topic，同样事件应通过
        tx.send_replace(other_topic);
        let task2 = TaskId::new();
        let mut meta2 = EventMeta::now();
        meta2.agent = Some(worker);
        meta2.task = Some(task2);
        meta2.topic_id = Some(other_topic);
        bus.publish(Event {
            meta: meta2,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Delivering,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "current=other 后应触发 task_done 注入");
        assert!(calls[0].2.contains("[TASK_DONE]"));
    }

    /// 跨 topic 的 milestone（AgentDead / AgentRequestReview / DeliverableProduced /
    /// ReviewRequestTimeout）依旧透传——决策 7 阈值：玄女总该知道门客死了 / 求审 /
    /// 交付完成，无论它属哪个 topic。
    #[tokio::test]
    async fn bridge_forwards_cross_topic_milestone_events() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let current = TopicId::new();
        let other = TopicId::new();
        let (_tx, rx) = watch::channel(current);
        let _h = SystemEventBridge::spawn_with_topic(
            mock.clone(),
            bus.clone(),
            xuannv,
            empty_lookup(),
            rx,
        );

        // AgentDead 跨 topic 仍透传
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.topic_id = Some(other);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "test crash".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "AgentDead 跨 topic 应透传");
        assert!(
            calls[0].2.contains("已下线"),
            "应是 death prompt：{:?}",
            calls[0].2
        );
    }

    /// 老事件（meta.topic_id=None）视作 general：current 是 general 时透传，
    /// current 非 general 时也透传（None 不参与 filter，保持向后兼容）。
    #[tokio::test]
    async fn bridge_passes_through_legacy_events_without_topic_id() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let custom_topic = TopicId::new();
        let (_tx, rx) = watch::channel(custom_topic);
        let _h = SystemEventBridge::spawn_with_topic(
            mock.clone(),
            bus.clone(),
            xuannv,
            empty_lookup(),
            rx,
        );

        // 老事件：meta.topic_id=None（模拟 Phase 1 之前持久化的事件回放）
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        // 故意不设 topic_id
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Delivering,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert!(
            !calls.is_empty(),
            "老事件 meta.topic_id=None 应透传，不参与 filter"
        );
    }

    // ─── 块3：分身池路由（跨 topic 串味根因修复 357da78a）──────────────

    /// 核心串味回归：池里 topicA→分身A、topicB→分身B。发 topicA 门客的
    /// AgentRequestReview（meta.topic_id=A）→ **只**注入分身A，分身B 零打扰。
    #[tokio::test]
    async fn worker_milestone_routes_to_owning_topic_clone_not_others() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let clone_a = AgentId::new();
        let clone_b = AgentId::new();
        let general = AgentId::new();
        let worker = AgentId::new();
        let topic_a = TopicId::new();
        let topic_b = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        // 池映射：topicA→分身A、topicB→分身B。
        let mut map = std::collections::HashMap::new();
        map.insert(topic_a, clone_a);
        map.insert(topic_b, clone_b);
        let (_tx, rx) = watch::channel(map);

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        // topicA 门客的求审 milestone。
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        meta.topic_id = Some(topic_a);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: fuxi_core::DeliverableKind::ResearchSummary,
                summary: "topicA 的活干完了".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "应且仅注入一次");
        assert_eq!(calls[0].0, clone_a, "里程碑应路由到归属 topicA 的分身A");
        assert!(
            calls.iter().all(|(t, _, _)| *t != clone_b),
            "分身B（topicB）一次都不该被打扰：{calls:?}"
        );
        assert!(
            calls.iter().all(|(t, _, _)| *t != general),
            "也不该兜底打到 general 分身"
        );
    }

    /// 无 topic_id 的旧事件 / 平台级事件 → 兜底 general 分身。
    #[tokio::test]
    async fn event_without_topic_falls_back_to_general_clone() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let general = AgentId::new();
        let other_clone = AgentId::new();
        let worker = AgentId::new();
        let other_topic = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        // 池里只有 other_topic→other_clone；general 不在池里（靠 xuannv_watch 镜像兜底）。
        let mut map = std::collections::HashMap::new();
        map.insert(other_topic, other_clone);
        let (_tx, rx) = watch::channel(map);

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 无 topic_id 的 AgentDead（平台级 / 老事件）。
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        // 故意不设 topic_id
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "crash".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, general, "无 topic 事件应兜底 general 分身");
    }

    /// 块4 核心回归：归属 topic 分身 dormant（不在池）→ 里程碑落持久队列
    /// （enqueue_pending 被调一次、topic 正确），**不**注入任何分身（不误打别 topic）。
    #[tokio::test]
    async fn bridge_dormant_milestone_enqueues_pending() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let general = AgentId::new();
        let live_clone = AgentId::new();
        let worker = AgentId::new();
        let live_topic = TopicId::new();
        let dormant_topic = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        // 池里只有 live_topic→live_clone；dormant_topic 无活分身。
        let mut map = std::collections::HashMap::new();
        map.insert(live_topic, live_clone);
        let (_tx, rx) = watch::channel(map);

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        // dormant_topic 门客的 milestone——归属分身不在池里。
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        meta.topic_id = Some(dormant_topic);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: fuxi_core::DeliverableKind::ResearchSummary,
                summary: "dormant topic 的活".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        // 等 enqueue 落库。
        for _ in 0..100 {
            if !mock.enqueue_snapshot().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let enq = mock.enqueue_snapshot().await;
        assert_eq!(enq.len(), 1, "dormant 里程碑应入队一次：{enq:?}");
        assert_eq!(enq[0].0, dormant_topic, "入队 topic 应为归属 dormant topic");
        assert_eq!(enq[0].2, "review_request", "origin 应为 review_request");
        assert!(
            enq[0].1.contains("[REVIEW_REQUEST]"),
            "落库的是最终 prompt 文本"
        );
        // 不该注入任何分身（不误打 live_clone / general）。
        assert!(
            mock.snapshot().await.is_empty(),
            "dormant 里程碑不应注入任何分身：{:?}",
            mock.snapshot().await
        );
    }

    /// 块4 对照：**活分身**的里程碑走注入、**不**入队（enqueue 只给 dormant）。
    #[tokio::test]
    async fn bridge_active_clone_milestone_injects_not_enqueues() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let general = AgentId::new();
        let live_clone = AgentId::new();
        let worker = AgentId::new();
        let live_topic = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let mut map = std::collections::HashMap::new();
        map.insert(live_topic, live_clone);
        let (_tx, rx) = watch::channel(map);

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        meta.topic_id = Some(live_topic);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: fuxi_core::DeliverableKind::ResearchSummary,
                summary: "活分身 topic 的活".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(mock.snapshot().await[0].0, live_clone, "活分身应被注入");
        assert!(
            mock.enqueue_snapshot().await.is_empty(),
            "活分身路径不该入队"
        );
    }

    /// 块4 噪音过滤：dormant topic 的 idle_ttl AgentDead 不入队（同活路径 silent）。
    #[tokio::test]
    async fn bridge_dormant_idle_ttl_agent_dead_not_enqueued() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let general = AgentId::new();
        let worker = AgentId::new();
        let dormant_topic = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        // 空池——dormant_topic 无活分身。
        let (_tx, rx) = watch::channel(std::collections::HashMap::new());

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.topic_id = Some(dormant_topic);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "idle_ttl".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            mock.enqueue_snapshot().await.is_empty(),
            "idle_ttl 正常回收不该入队（噪音过滤）"
        );
    }

    /// 块5 步7.7 跨块集成回归（仿今天玄女卡死场景）：dormant topic 分身在它睡着期间
    /// 门客完工 → bridge **既** enqueue 落库 **又** 触发 ensure_xuannv_for_topic respawn，
    /// 顺序是先 enqueue 再 respawn（spawn 内 drain 一定看得到刚入队的信号）。
    #[tokio::test]
    async fn dormant_milestone_enqueues_then_triggers_respawn() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let general = AgentId::new();
        let worker = AgentId::new();
        let dormant_topic = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;
        // 空池——dormant_topic 无活分身（模拟分身已被 idle_gc dormant 回收）。
        let (_tx, rx) = watch::channel(std::collections::HashMap::new());

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(task);
        meta.topic_id = Some(dormant_topic);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentRequestReview {
                agent: worker,
                task,
                deliverable_kind: fuxi_core::DeliverableKind::ResearchSummary,
                summary: "分身睡着期间门客干完了".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        // 等 enqueue + respawn 都发生。
        for _ in 0..100 {
            if !mock.enqueue_snapshot().await.is_empty()
                && !mock.respawn_snapshot().await.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let enq = mock.enqueue_snapshot().await;
        let resp = mock.respawn_snapshot().await;
        assert_eq!(enq.len(), 1, "dormant 里程碑应落库一次");
        assert_eq!(enq[0].0, dormant_topic);
        assert_eq!(
            resp.as_slice(),
            &[dormant_topic],
            "应触发该 topic 的 respawn 补发"
        );
        // 不注入任何活分身（不误打 general / 别 topic）。
        assert!(
            mock.snapshot().await.is_empty(),
            "dormant 路径不该直接注入分身"
        );
    }

    /// 块5 步7.7 串味隔离 e2e（两 topic 各有活分身）：topic A / B 各自 worker 完工
    /// milestone 严格只注入自己 topic 的分身，互不串。357da78a 串味的核心治愈断言。
    ///
    /// 注：本测覆盖 milestone→归属分身 路由（已实装）。worker-dispatch 让门客事件
    /// **带上**正确 meta.topic_id 那条链是 7.5（DEFER follow-up）——本测直接 stamp
    /// meta.topic_id 模拟 7.5 生效后的世界，验证路由层隔离正确。
    #[tokio::test]
    async fn two_topics_milestones_stay_isolated_no_cross_talk() {
        use fuxi_core::TopicId;
        let bus = EventBus::with_memory_store().await.unwrap();
        let clone_a = AgentId::new();
        let clone_b = AgentId::new();
        let general = AgentId::new();
        let worker_a = AgentId::new();
        let worker_b = AgentId::new();
        let topic_a = TopicId::new();
        let topic_b = TopicId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker_a, "luban").await;
        mock.set_role(worker_b, "codex").await;

        let mut map = std::collections::HashMap::new();
        map.insert(topic_a, clone_a);
        map.insert(topic_b, clone_b);
        let (_tx, rx) = watch::channel(map);

        let _h = SystemEventBridge::spawn_with_pool_for_test(
            mock.clone(),
            bus.clone(),
            general,
            rx,
            empty_lookup(),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        for (worker, topic, summary) in
            [(worker_a, topic_a, "A 的活"), (worker_b, topic_b, "B 的活")]
        {
            let task = TaskId::new();
            let mut meta = EventMeta::now();
            meta.agent = Some(worker);
            meta.task = Some(task);
            meta.topic_id = Some(topic);
            bus.publish(Event {
                meta,
                kind: EventKind::AgentRequestReview {
                    agent: worker,
                    task,
                    deliverable_kind: fuxi_core::DeliverableKind::ResearchSummary,
                    summary: summary.into(),
                    artifact_ref: None,
                },
            })
            .expect("publish");
        }

        wait_call(&mock, 2).await;
        tokio::time::sleep(Duration::from_millis(40)).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 2, "两条 milestone 各注入一次：{calls:?}");
        let a_targets: Vec<_> = calls
            .iter()
            .filter(|(_, _, text)| text.contains("A 的活"))
            .map(|(t, _, _)| *t)
            .collect();
        let b_targets: Vec<_> = calls
            .iter()
            .filter(|(_, _, text)| text.contains("B 的活"))
            .map(|(t, _, _)| *t)
            .collect();
        assert_eq!(a_targets, vec![clone_a], "A 的完工只该到分身A");
        assert_eq!(b_targets, vec![clone_b], "B 的完工只该到分身B");
        assert!(
            calls.iter().all(|(t, _, _)| *t != general),
            "活分身路由不该兜底 general：{calls:?}"
        );
    }
}
