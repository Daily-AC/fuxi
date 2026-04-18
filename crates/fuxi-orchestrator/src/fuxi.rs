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

    /// 把一个已经实例化的 `Agent` 直接塞进 shelf——主要给测试 / stub agent
    /// 用（也是未来 adapter 外置时的扩展点）。
    ///
    /// WHY：`spawn_worker` 走的是"我们这边根据 WorkerKind 去 spawn 适配器"
    /// 的路径；但有时调用方已经有一个现成的 `Arc<dyn Agent>`（比如外部 A2A
    /// endpoint 包装、测试 stub），这时不再需要我们 spawn，只需要登记。
    ///
    /// 返回的 id 以 `agent.card().id` 为准。
    pub async fn insert_agent(
        &self,
        agent: Arc<dyn Agent>,
        worktree: Option<fuxi_core::workspace::WorkspaceHandle>,
    ) -> AgentId {
        let card = agent.card().clone();
        let id = card.id;
        let entry = ShelfEntry {
            card,
            agent,
            status: ShelfStatus::Idle,
            worktree,
        };
        self.shelf.insert(entry).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(id);
        self.bus
            .publish(Event {
                meta,
                kind: EventKind::AgentReady {
                    endpoint: "externally-managed".into(),
                },
            })
            .ok();
        id
    }

    /// 列出所有已注册门客的 card。
    pub async fn list_workers(&self) -> Vec<fuxi_core::agent::AgentCard> {
        self.shelf.list_cards().await
    }

    /// 拉起一个新门客。
    ///
    /// 流程：
    /// 1. 发 `AgentSpawning` 事件到 bus；
    /// 2. 若 cfg.allocate_worktree：向 workspace 申请一个 worktree；
    /// 3. 调对应适配器的 `launch`；
    /// 4. 把 entry 登记到 shelf；
    /// 5. 发 `AgentReady` 事件（含 endpoint pid:...）。
    ///
    /// 失败时已经分配出去的 worktree 会尽量回滚；回滚本身失败只 log、不抛——
    /// 此时 worktree 成"垃圾"，需要人工或 P3 的 GC 扫。
    pub async fn spawn_worker(&self, profile: AgentProfile, kind: WorkerKind) -> Result<AgentId> {
        let agent_id = AgentId::new();
        info!(agent = %agent_id, role = %profile.role, "spawn worker");

        // 1. 发 AgentSpawning。
        let meta = {
            let mut m = EventMeta::now();
            m.agent = Some(agent_id);
            m
        };
        let cli_tag = match &kind {
            WorkerKind::Cc(_) => "claude-code".to_string(),
        };
        self.bus
            .publish(Event {
                meta: meta.clone(),
                kind: EventKind::AgentSpawning {
                    role: profile.role.clone(),
                    cli: cli_tag.clone(),
                },
            })
            .ok();

        // 2. worktree（可选）。
        let worktree = if self.cfg.allocate_worktree {
            match self.workspace.create(agent_id, &self.cfg.base_branch).await {
                Ok(h) => Some(h),
                Err(e) => {
                    warn!(error = %e, "分配 worktree 失败，继续使用当前 cwd");
                    None
                }
            }
        } else {
            None
        };

        // 3. 构造 agent。launch 时若指定了 cwd，需要把 worktree_path 塞进去；
        //    不覆写已显式设置的 cfg.cwd。
        let launch_result = match kind {
            WorkerKind::Cc(mut cc_cfg) => {
                if let (None, Some(h)) = (cc_cfg.cwd.as_ref(), worktree.as_ref()) {
                    cc_cfg.cwd = Some(h.worktree_path.clone());
                }
                CcAgent::launch(profile.clone(), cc_cfg)
            }
        };
        match launch_result {
            Ok(a) => {
                // 用 agent 自己生成的 id 覆盖我们的——保持一致性。
                // （CcAgent::launch 内部会产生自己的 AgentId；这里 agent_id
                // 应当以 agent.card().id 为准。）
                let actual_id = a.card().id;
                let pid_hint = a.card().endpoint.clone();

                let entry = ShelfEntry {
                    card: a.card().clone(),
                    agent: Arc::new(a),
                    status: ShelfStatus::Idle,
                    worktree: worktree.clone(),
                };
                self.shelf.insert(entry).await;

                // 发 AgentReady。
                let mut ready_meta = EventMeta::now();
                ready_meta.agent = Some(actual_id);
                self.bus
                    .publish(Event {
                        meta: ready_meta,
                        kind: EventKind::AgentReady { endpoint: pid_hint },
                    })
                    .ok();

                Ok(actual_id)
            }
            Err(e) => {
                // launch 失败——回滚 worktree。
                if let Some(h) = worktree.as_ref()
                    && let Err(cleanup) = self.workspace.destroy(h).await
                {
                    warn!(error = %cleanup, "回滚 worktree 失败（留档）");
                }
                let mut dead_meta = EventMeta::now();
                dead_meta.agent = Some(agent_id);
                self.bus
                    .publish(Event {
                        meta: dead_meta,
                        kind: EventKind::AgentDead {
                            cause: format!("launch failed: {e}"),
                        },
                    })
                    .ok();
                Err(OrchestratorError::Core(e))
            }
        }
    }

    /// 给指定门客派一个 task，事件流自动 republish 到 EventBus。
    ///
    /// 返回时只保证 task 已经递交——完成与否靠订阅 EventBus 上的
    /// `TaskStateChanged { to: Done | Blocked | Cancelled }` 判断。
    pub async fn dispatch(&self, agent_id: AgentId, task: Task) -> Result<()> {
        let agent = self
            .shelf
            .get_agent(agent_id)
            .await
            .ok_or(OrchestratorError::AgentNotFound(agent_id))?;

        // 标记 busy。完成事件由 pump 任务观察到后标回 idle。
        self.shelf.set_status(agent_id, ShelfStatus::Busy).await;

        let mut rx = agent.dispatch(task).await?;
        let bus = self.bus.clone();
        let shelf = self.shelf.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let is_terminal = matches!(
                    ev.kind,
                    EventKind::TaskStateChanged {
                        to: fuxi_core::task::TaskState::Done
                            | fuxi_core::task::TaskState::Cancelled
                            | fuxi_core::task::TaskState::Blocked,
                        ..
                    }
                );
                if bus.publish(ev).is_err() {
                    break;
                }
                if is_terminal {
                    shelf.set_status(agent_id, ShelfStatus::Idle).await;
                    break;
                }
            }
            debug!(agent = %agent_id, "dispatch pump 退出");
        });

        Ok(())
    }

    /// 按角色挑一个空闲门客派任务；没空闲就先 spawn 一个再派。
    ///
    /// `kind_for_spawn` 决定 spawn 时用哪种适配器（cc/codex/...）。
    pub async fn dispatch_to_any(
        &self,
        role: &str,
        task: Task,
        profile_template: AgentProfile,
        kind_for_spawn: WorkerKind,
    ) -> Result<AgentId> {
        let chosen = if let Some(id) = self.shelf.find_idle_by_role(role).await {
            debug!(agent = %id, role, "复用空闲门客");
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

    /// 停掉所有门客 + 对应 worktree。
    pub async fn shutdown(&self) -> Result<()> {
        let cards = self.shelf.list_cards().await;
        info!(count = cards.len(), "fuxi shutdown: 关闭所有门客");
        for card in cards {
            if let Some(entry) = self.shelf.take(card.id).await {
                if let Err(e) = entry.agent.shutdown().await {
                    warn!(agent = %card.id, error = %e, "agent shutdown 出错");
                }
                if let Some(h) = entry.worktree.as_ref()
                    && let Err(e) = self.workspace.destroy(h).await
                {
                    warn!(agent = %card.id, error = %e, "worktree destroy 出错");
                }
                let mut meta = EventMeta::now();
                meta.agent = Some(card.id);
                self.bus
                    .publish(Event {
                        meta,
                        kind: EventKind::AgentShuttingDown {
                            reason: "fuxi shutdown".into(),
                        },
                    })
                    .ok();
            }
        }
        Ok(())
    }
}
