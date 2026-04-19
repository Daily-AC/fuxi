//! `Fuxi`——玄女的主要入口。
//!
//! 生命周期：
//! 1. `Fuxi::new(bus, workspace)` 零门客启动。
//! 2. `spawn_worker(profile, WorkerKind::Cc(cfg))` 拉起具体门客，返回 `AgentId`。
//! 3. `dispatch(id, task)` 把 task 丢给指定门客——事件自动 republish 到 bus。
//! 4. `dispatch_to_any(role, task)` 按角色找空闲门客或拉起新的。
//! 5. `shutdown()` 把所有门客 + 对应 worktree 按顺序销毁。
//!
//! 所有 mutating 方法是 `&self` 而非 `&mut self`——内部用 Arc+RwLock/Mutex。
//! 这样 `Arc<Fuxi>` 可以被多个后台 task 安全共享（CLI 的 REPL、A2A server 的
//! handler、世界模型 watcher 会一起持它）。

use crate::error::{OrchestratorError, Result};
use crate::registry::{Shelf, ShelfEntry, ShelfStatus};
use fuxi_agent_cc::{CcAgent, CcLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::AgentId;
use fuxi_core::task::Task;
use fuxi_core::workspace::Workspace;
use fuxi_events::EventBus;
use fuxi_workspace::GitWorktreeWorkspace;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// `Fuxi` 的可调参数。
#[derive(Debug, Clone)]
pub struct FuxiConfig {
    /// spawn 新门客时是否给它分配一个独立 worktree。默认 true——
    /// 这是我们三件套的基础。关掉主要给测试/玩具场景。
    pub allocate_worktree: bool,
    /// worktree 基于哪个 branch 切出。默认 "main"。
    pub base_branch: String,
}

impl Default for FuxiConfig {
    fn default() -> Self {
        Self {
            allocate_worktree: true,
            base_branch: "main".to_string(),
        }
    }
}

/// 支持 spawn 的门客种类。暂时只有 cc；codex/gemini/opencode 随其适配器
/// 完成后加分支即可。
#[derive(Debug, Clone)]
pub enum WorkerKind {
    /// Claude Code 门客，带启动参数。
    Cc(CcLaunchConfig),
}

impl WorkerKind {
    /// 对应到 `AgentProfile.cli` / `AgentSpawning.cli` 的文本标签。
    ///
    /// WHY 独立方法：P2 还会加 codex/gemini 的 variant；集中在此避免每个调用点
    /// 都 match 一次 `to_string`。
    pub fn cli_tag(&self) -> &'static str {
        match self {
            WorkerKind::Cc(_) => "claude-code",
        }
    }
}

/// 玄女主体。
pub struct Fuxi {
    bus: EventBus,
    workspace: Arc<GitWorktreeWorkspace>,
    shelf: Arc<Shelf>,
    cfg: FuxiConfig,
}

impl Fuxi {
    /// 默认配置启动。
    pub fn new(bus: EventBus, workspace: Arc<GitWorktreeWorkspace>) -> Self {
        Self::with_config(bus, workspace, FuxiConfig::default())
    }

    /// 自定义配置启动。
    pub fn with_config(
        bus: EventBus,
        workspace: Arc<GitWorktreeWorkspace>,
        cfg: FuxiConfig,
    ) -> Self {
        Self {
            bus,
            workspace,
            shelf: Arc::new(Shelf::new()),
            cfg,
        }
    }

    /// 已注册门客数。
    pub async fn worker_count(&self) -> usize {
        self.shelf.len().await
    }

    /// 查一个门客当前的 shelf 状态（Idle/Busy/Dead）；不存在返回 None。
    pub async fn status_of(&self, id: AgentId) -> Option<ShelfStatus> {
        self.shelf.status_of(id).await
    }

    /// 列出所有已注册门客的 card。
    pub async fn list_workers(&self) -> Vec<fuxi_core::agent::AgentCard> {
        self.shelf.list_cards().await
    }

    /// 把一个已经实例化的 `Agent` 直接塞进 shelf——主要给测试 / stub agent
    /// 用（也是未来 adapter 外置时的扩展点）。
    ///
    /// WHY：`spawn_worker` 走的是"我们这边根据 WorkerKind 去 spawn 适配器"
    /// 的路径；但有时调用方已经有一个现成的 `Arc<dyn Agent>`（比如外部 A2A
    /// endpoint 包装、测试 stub），这时不再需要我们 spawn，只需要登记。
    ///
    /// 返回的 id 以 `agent.card().id` 为准。生命周期事件（Spawning + Ready）
    /// 都会打到 bus 上，**不得**跳过——公理 #1。
    pub async fn insert_agent(
        &self,
        agent: Arc<dyn Agent>,
        worktree: Option<fuxi_core::workspace::WorkspaceHandle>,
    ) -> AgentId {
        let id = agent.card().id;
        // 补发 AgentSpawning 让生命周期事件闭合——外部托管不等于绕过公理。
        self.publish_with_agent(
            id,
            EventKind::AgentSpawning {
                role: agent.card().profile.role.clone(),
                cli: agent.card().profile.cli.clone(),
            },
        );
        self.register_ready(agent, worktree, "externally-managed".into())
            .await;
        id
    }

    /// 拉起一个新门客。
    ///
    /// 流程：
    /// 1. 发 `AgentSpawning`；
    /// 2. cfg.allocate_worktree=true 时向 workspace 申请 worktree（失败即退出，
    ///    不静默 fallback——公理层的"独立 worktree"是锚点场景的前置）；
    /// 3. 调对应适配器的 `launch_with_id(agent_id, ...)`——让 id 唯一真相源是
    ///    玄女本身；
    /// 4. shelf 登记 + 发 `AgentReady`。
    ///
    /// 失败时已分配的 worktree 会被回滚（destroy 失败只 warn，不让清理错误掩盖
    /// 原始 launch 错误）；同时发 `AgentDead { cause: launch failed: ... }`。
    pub async fn spawn_worker(&self, profile: AgentProfile, kind: WorkerKind) -> Result<AgentId> {
        let agent_id = AgentId::new();
        info!(agent = %agent_id, role = %profile.role, "spawn worker");

        // 1. AgentSpawning。
        self.publish_with_agent(
            agent_id,
            EventKind::AgentSpawning {
                role: profile.role.clone(),
                cli: kind.cli_tag().to_string(),
            },
        );

        // 2. worktree（可选，失败硬返）。
        let worktree = if self.cfg.allocate_worktree {
            Some(
                self.workspace
                    .create(agent_id, &self.cfg.base_branch)
                    .await?,
            )
        } else {
            None
        };

        // 3. 适配器 launch。
        let launch_result = match kind {
            WorkerKind::Cc(mut cc_cfg) => {
                if let (None, Some(h)) = (cc_cfg.cwd.as_ref(), worktree.as_ref()) {
                    cc_cfg.cwd = Some(h.worktree_path.clone());
                }
                CcAgent::launch_with_id(agent_id, profile.clone(), cc_cfg).await
            }
        };
        match launch_result {
            Ok(a) => {
                let endpoint_hint = a.card().endpoint.clone();
                let agent: Arc<dyn Agent> = Arc::new(a);
                let id = self.register_ready(agent, worktree, endpoint_hint).await;
                debug_assert_eq!(id, agent_id, "launch_with_id 应保证 id 一致");
                Ok(agent_id)
            }
            Err(e) => {
                // 回滚 worktree——destroy 失败只 warn，原始错误才是重点。
                if let Some(h) = worktree.as_ref()
                    && let Err(cleanup) = self.workspace.destroy(h).await
                {
                    warn!(error = %cleanup, "回滚 worktree 失败（留档）");
                }
                self.publish_with_agent(
                    agent_id,
                    EventKind::AgentDead {
                        cause: format!("launch failed: {e}"),
                    },
                );
                Err(OrchestratorError::Core(e))
            }
        }
    }

    /// 给指定门客派一个 task，事件流自动 republish 到 EventBus。
    ///
    /// 返回时只保证 task 已经递交——完成与否靠订阅 EventBus 上的
    /// `TaskStateChanged { to: Done | Cancelled }` 或 `AgentDead` 判断。
    /// `Blocked` 是可恢复态（允许 Blocked → Ready），故**不**视为终结。
    ///
    /// 保证：**pump task 退出时无论何种原因**（见到终结事件 / channel 被 agent
    /// 提前关闭 / bus 关闭），shelf 状态必然回到 Idle——避免门客被永久锁在 Busy。
    pub async fn dispatch(&self, agent_id: AgentId, task: Task) -> Result<()> {
        let agent = self
            .shelf
            .get_agent(agent_id)
            .await
            .ok_or(OrchestratorError::AgentNotFound(agent_id))?;

        self.shelf.set_status(agent_id, ShelfStatus::Busy).await;

        let mut rx = agent.dispatch(task).await?;
        let bus = self.bus.clone();
        let shelf = self.shelf.clone();
        tokio::spawn(async move {
            let mut seen_terminal = false;
            while let Some(ev) = rx.recv().await {
                // Blocked 是可恢复态（允许回 Ready），不列为 terminal。
                let is_terminal = matches!(
                    &ev.kind,
                    EventKind::TaskStateChanged {
                        to: fuxi_core::task::TaskState::Done
                            | fuxi_core::task::TaskState::Cancelled,
                        ..
                    } | EventKind::TaskDelivered { .. }
                        | EventKind::TaskCancelled { .. }
                        | EventKind::AgentDead { .. }
                );
                if bus.publish(ev).is_err() {
                    warn!(agent = %agent_id, "event bus 已关闭，pump 退出");
                    break;
                }
                if is_terminal {
                    seen_terminal = true;
                    break;
                }
            }
            // 无论怎么退出都摊回 Idle——channel 被 agent 提前关也算"不忙"。
            shelf.set_status(agent_id, ShelfStatus::Idle).await;
            debug!(agent = %agent_id, seen_terminal, "dispatch pump 退出");
        });

        Ok(())
    }

    /// 介入——向某个门客发话。
    ///
    /// - `append`：追加一条 user message，门客下一 turn 看到（stdio/WS 都能做）
    /// - `interrupt`：打断当前 turn 再追加（依赖 WS 模式的 control_request/interrupt）
    ///
    /// v0.1 下 task_id 是"最近一次 dispatch"的——cc 适配器当前忽略这个参数，
    /// 直接用它内部跟踪的 `current_task`。
    ///
    /// 薄片 I 将在此基础上加：玄女事件发布（UserInterventionSent）、状态机
    /// (task_intervention_applied)、以及辨识"用户说'停/换方向'"的 NLU 层。
    /// 这里只做 wire 层的"把话送到"。
    pub async fn intervene(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
    ) -> Result<()> {
        let agent = self
            .shelf
            .get_agent(agent_id)
            .await
            .ok_or(OrchestratorError::AgentNotFound(agent_id))?;

        // cc 忽略 task_id，这里传随机 id 兼容 trait 签名
        let dummy_task = fuxi_core::id::TaskId::new();

        if interrupt_first {
            info!(agent = %agent_id, "intervene: 打断式");
            agent.cancel(dummy_task).await?;
        } else {
            info!(agent = %agent_id, "intervene: 追加式");
        }
        agent.send_message(dummy_task, text).await?;
        Ok(())
    }

    /// 按角色挑一个空闲门客派任务；没空闲就先 spawn 一个再派。
    ///
    /// 使用 `claim_idle_by_role` 原子地"找+占"，防止并发 `dispatch_to_any`
    /// 把同一个空闲门客派两次（TOCTOU）。
    pub async fn dispatch_to_any(
        &self,
        role: &str,
        task: Task,
        profile_template: AgentProfile,
        kind_for_spawn: WorkerKind,
    ) -> Result<AgentId> {
        let chosen = if let Some(id) = self.shelf.claim_idle_by_role(role).await {
            debug!(agent = %id, role, "原子复用空闲门客");
            id
        } else {
            debug!(role, "无空闲门客，spawn 新的");
            let mut p = profile_template;
            p.role = role.to_string();
            self.spawn_worker(p, kind_for_spawn).await?
        };
        self.dispatch(chosen, task).await?;
        Ok(chosen)
    }

    /// 停掉所有门客 + 对应 worktree。幂等——连续调多次等价于调一次。
    ///
    /// 事件顺序（每个门客）：`AgentShuttingDown`（动作前） → agent.shutdown +
    /// worktree.destroy → `AgentDead`（动作后）。确保 Firehose 能把生命周期完整收尾。
    pub async fn shutdown(&self) -> Result<()> {
        let cards = self.shelf.list_cards().await;
        info!(count = cards.len(), "fuxi shutdown: 关闭所有门客");
        for card in cards {
            let Some(entry) = self.shelf.take(card.id).await else {
                continue;
            };
            self.publish_with_agent(
                card.id,
                EventKind::AgentShuttingDown {
                    reason: "fuxi shutdown".into(),
                },
            );
            if let Err(e) = entry.agent.shutdown().await {
                warn!(agent = %card.id, error = %e, "agent shutdown 出错");
            }
            if let Some(h) = entry.worktree.as_ref()
                && let Err(e) = self.workspace.destroy(h).await
            {
                warn!(agent = %card.id, error = %e, "worktree destroy 出错");
            }
            self.publish_with_agent(
                card.id,
                EventKind::AgentDead {
                    cause: "fuxi shutdown".into(),
                },
            );
        }
        Ok(())
    }

    // ───────── 内部 helper ─────────

    /// 构造带 `agent` 字段的 `EventMeta` 并发到 bus——忽略 publish 的 `Err`
    /// （bus 关闭时）；调用方已经没法对此做什么了。
    fn publish_with_agent(&self, agent: AgentId, kind: EventKind) {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        let _ = self.bus.publish(Event { meta, kind });
    }

    /// 把 agent 登记到 shelf 并发 `AgentReady`。返回 card id。
    ///
    /// `AgentSpawning` 由调用方单独发——spawn_worker 在 launch 前就发了，
    /// insert_agent 也会在进来时补一条；这里只处理 "就绪后" 的部分。
    async fn register_ready(
        &self,
        agent: Arc<dyn Agent>,
        worktree: Option<fuxi_core::workspace::WorkspaceHandle>,
        endpoint_hint: String,
    ) -> AgentId {
        let card = agent.card().clone();
        let id = card.id;
        self.shelf
            .insert(ShelfEntry {
                card,
                agent,
                status: ShelfStatus::Idle,
                worktree,
            })
            .await;
        self.publish_with_agent(
            id,
            EventKind::AgentReady {
                endpoint: endpoint_hint,
            },
        );
        id
    }
}
