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
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// AgentRequestReview retry 退避序列（毫秒，指数退避）。
///
/// 暴露 `pub(crate)` 让单测 + delta 的 e2e fixture 直接复用——若 timeout
/// 预算调整，改这一处即可（delta 算 sum + buffer 作 timeout 等待预算）。
/// 当前 sum = 1700ms（200+500+1000），delta 用 2.5s 留余量。
pub(crate) const REVIEW_RETRY_BACKOFF_MS: &[u64] = &[200, 500, 1000];

/// 内部 role 黑名单：这些门客的 [`EventKind::AgentDead`] **不抄送**给玄女。
///
/// 为什么：extractor 是 M2.5 自动后台跑的"幕后工"——其生死属平台级
/// 自管理（spawn/reap 由 hook 控制），玄女不应被这种噪音占 attention。
/// 历史上 TaskStateChanged Done 路径也走这层过滤，但 Decision 13 之后
/// TaskDone 整段不再触发 intervene，故仅 AgentDead 一处用得到。
const INTERNAL_ROLES: &[&str] = &["extractor"];

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
        )
        .await
    }

    async fn role_of(&self, agent_id: AgentId) -> Option<String> {
        self.list_workers()
            .await
            .into_iter()
            .find(|c| c.id == agent_id)
            .map(|c| c.profile.role.clone())
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

fn build_cc_prompt(to_worker: AgentId, role: &str, text: &str) -> String {
    format!(
        "[CC] 用户直接对门客 {to_worker}（role={role}）说：「{text}」。\n\n你未被点名，仅为抄送留痕（公理 #2：你永远有知情权）。无需主动回话，除非判断需介入。"
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
) -> Result<()> {
    // 首发——若成功直接返回，不进 retry。
    let mut last_err = match intervener.intervene(target, true, prompt).await {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    for (idx, &ms) in backoff_ms.iter().enumerate() {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        match intervener.intervene(target, true, prompt).await {
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
    pub fn spawn(
        fuxi: Arc<Fuxi>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        let intervener: Arc<dyn Intervener> = fuxi;
        Self::spawn_with(intervener, bus, xuannv_id, trigger_lookup)
    }

    /// 测试/内部路径——任意 [`Intervener`] 注入。
    pub fn spawn_with(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        Self::spawn_inner(
            intervener,
            bus,
            xuannv_id,
            trigger_lookup,
            REVIEW_RETRY_BACKOFF_MS,
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
        Self::spawn_inner(intervener, bus, xuannv_id, trigger_lookup, backoff_ms)
    }

    fn spawn_inner(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
        backoff_ms: &'static [u64],
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
            if let Err(e) = intervener.intervene(xuannv_id, false, &prompt).await {
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
                .intervene(xuannv_id, interrupt_first, &prompt)
                .await
            {
                warn!(error = %e, "bridge: intervene(AgentDead) 失败");
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
            let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
            info!(
                %agent,
                %role,
                kind = deliverable_kind_tag(deliverable_kind),
                lag_ms,
                "bridge: 转发 AgentRequestReview 到玄女（带 retry）"
            );
            let prompt = build_request_review_prompt(
                agent,
                &role,
                deliverable_kind,
                summary,
                artifact_ref.as_deref(),
            );
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
            if let Err(e) =
                try_intervene_with_retry(intervener, xuannv_id, &prompt, backoff_ms).await
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
                .intervene(xuannv_id, interrupt_first, &prompt)
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
            if let Err(e) = intervener.intervene(xuannv_id, false, &prompt).await {
                warn!(error = %e, "bridge: intervene(OrchestratorCcReceived) 失败");
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

        async fn archive_snapshot(
            &self,
        ) -> Vec<(fuxi_core::ProjectId, TaskId, fuxi_core::ArchiveReason)> {
            self.archive_calls.lock().await.clone()
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
        assert!(text.contains("[CC]"), "prompt 应含 [CC] 标识: {text}");
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

    /// Decision 13 后 TaskStateChanged → Done/Cancelled 不再抄送玄女（任何 role）。
    /// 中间过程 silent，门客需用 AgentRequestReview 主动 nudge 才会占 attention。
    /// 此 test 替代历史的 extractor_task_done_is_not_copied_to_xuannv 回归点。
    #[tokio::test]
    async fn task_done_no_longer_copies_to_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        meta.task = Some(fuxi_core::id::TaskId::new());
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Delivering,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "Decision 13 后 TaskDone 默认 silent，门客需主动 AgentRequestReview"
        );
    }

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
        let result = try_intervene_with_retry(&*mock, xuannv, "test prompt", &[1, 2, 4]).await;
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

        let result = try_intervene_with_retry(&*mock, xuannv, "test prompt", &[1, 2, 4]).await;
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
}
