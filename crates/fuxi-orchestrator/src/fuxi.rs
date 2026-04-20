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
use futures_util::StreamExt;
use fuxi_agent_cc::{CcAgent, CcLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::AgentId;
use fuxi_core::task::Task;
use fuxi_core::workspace::Workspace;
use fuxi_events::EventBus;
use fuxi_workspace::GitWorktreeWorkspace;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
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
    /// 顶层玄女 agent id——repl 启动 spawn 后通过 `set_xuannv` 告知。
    /// Why `Option`：`Fuxi::new` 零门客启动，早于任何 spawn；抄送路径
    /// 遇到 `None` 时 graceful skip，不强求设置。
    xuannv_id: Arc<RwLock<Option<AgentId>>>,
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
        let shelf = Arc::new(Shelf::new());
        let me = Self {
            bus: bus.clone(),
            workspace,
            shelf: shelf.clone(),
            cfg,
            xuannv_id: Arc::new(RwLock::new(None)),
        };
        // 死亡检测：Fuxi 自订阅 bus，看到 AgentDead 即把对应 shelf 条目翻 Dead。
        // why 放在这里：唯一拥有 shelf 写权限的地方；具体死亡检测源头（cc WS 关闭、
        // Fuxi::shutdown 主动发、外部 publish）全部汇入这一条路径。
        spawn_death_watcher(bus, shelf);
        me
    }

    /// 拿到 EventBus 的引用——给需要直接推事件的外部 caller 用
    /// （例如 daemon 处理 `Command::EmitEvent`）。
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// 已注册门客数。
    pub async fn worker_count(&self) -> usize {
        self.shelf.len().await
    }

    /// 告知 Fuxi 哪个 agent 是玄女——抄送路径用这个判定 target≠xuannv。
    /// 幂等：再次调用以最新值为准。
    pub async fn set_xuannv(&self, id: AgentId) {
        *self.xuannv_id.write().await = Some(id);
    }

    /// 读玄女 id——未设置返回 None。
    pub async fn xuannv_id(&self) -> Option<AgentId> {
        *self.xuannv_id.read().await
    }

    /// 读某门客分配的 worktree 路径——纯转发 shelf，供 TUI/CLI 用。
    pub async fn worktree_of(&self, id: AgentId) -> Option<PathBuf> {
        self.shelf.worktree_of(id).await
    }

    /// 克隆 shelf Arc——给 TUI 订阅者（只读观察 roster / worktree / 状态）。
    /// WHY 只暴露只读意图：shelf 的修改权掌握在 Fuxi 手里，TUI 不能直接 set_status。
    pub fn clone_shelf(&self) -> Arc<Shelf> {
        self.shelf.clone()
    }

    /// 发一条让贤（主对话权转交）请求——TUI 订阅后自动切 active。
    /// 不走门客 agent.send_message；只是事件广播，FIRE-AND-FORGET。
    pub fn request_handoff(
        &self,
        from: AgentId,
        to: AgentId,
        reason: String,
        brief: Option<String>,
    ) {
        let mut meta = EventMeta::now();
        meta.agent = Some(from);
        let _ = self.bus.publish(Event {
            meta,
            kind: EventKind::ConversationHandoffRequested {
                from,
                to,
                reason,
                brief,
            },
        });
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
                // 取出死亡信号接收端 → spawn 转发任务 → 死亡时 publish AgentDead。
                // 放在 Arc::new 之前——take_death_watch 是 `&CcAgent` 方法，
                // 装进 Arc<dyn Agent> 后就拿不动了。
                let death_rx = a.take_death_watch();
                if let Some(mut rx) = death_rx {
                    let bus = self.bus.clone();
                    tokio::spawn(async move {
                        if let Some(reason) = rx.recv().await {
                            let mut meta = EventMeta::now();
                            meta.agent = Some(agent_id);
                            let _ = bus.publish(Event {
                                meta,
                                kind: EventKind::AgentDead { cause: reason },
                            });
                        }
                    });
                }
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
    /// v0.1 薄片 I 承诺的三个事件：
    /// - `UserInterventionSent { target, mode, text }`  （入口）
    /// - `AgentInterrupted { reason }`   仅在 interrupt 模式下发
    /// - `TaskInterventionApplied { mode }`  wire 层确认
    ///
    /// cc 适配器忽略 task_id，这里传随机 id 兼容 trait 签名；事件上不挂
    /// task 维度（没有从 dispatch 回流最近 task 的路径）——v0.2 补上"最近
    /// dispatch 的 task 记忆"后再加。
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

        // 空闲门客自动退化成 dispatch（玄女 2026-04-20 诊断出的 bug）：
        // cc idle 状态下 active_tx=None，`send_message` 发进 WS 的响应没 receiver，
        // cc 的回复事件会被 drop —— 用户看起来"门客不理我"。
        // 正确处理是把这次 intervene 当作一次新 dispatch，cc 有 active_tx 接响应。
        // 语义上仍发一条 UserInterventionSent 事件（mode=append_via_dispatch）+ 抄送，
        // 让用户视角一致：他"对空闲门客说话"本就等同于派新活。
        let shelf_status = self.shelf.status_of(agent_id).await;
        if matches!(shelf_status, Some(ShelfStatus::Idle)) {
            info!(agent = %agent_id, "intervene on idle → auto-degrade to dispatch");
            let intervention_ev_id = {
                let mut meta = EventMeta::now();
                meta.agent = Some(agent_id);
                let id = meta.id;
                let _ = self.bus.publish(Event {
                    meta,
                    kind: EventKind::UserInterventionSent {
                        target: agent_id,
                        mode: "append_via_dispatch".to_string(),
                        text: text.to_string(),
                    },
                });
                id
            };
            let _ = agent; // 不再直接操作 agent，下面 dispatch 内部会再拿一次
            let task = Task::new("intervention", text);
            self.dispatch(agent_id, task).await?;
            // 抄送玄女
            let xuannv = self.xuannv_id().await;
            if let Some(xn) = xuannv
                && xn != agent_id
            {
                let mut meta = EventMeta::now();
                meta.agent = Some(xn);
                let _ = self.bus.publish(Event {
                    meta,
                    kind: EventKind::OrchestratorCcReceived {
                        from_user_to: agent_id,
                        text: text.to_string(),
                        original_intervention_id: intervention_ev_id,
                    },
                });
            }
            return Ok(());
        }

        let mode_str = if interrupt_first {
            "interrupt"
        } else {
            "append"
        };

        // 1. UserInterventionSent —— 入口事件，意图进入事件流
        // why 显式给 meta id：下面抄送事件需要引用它作为 original_intervention_id
        let intervention_ev_id = {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent_id);
            let id = meta.id;
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::UserInterventionSent {
                    target: agent_id,
                    mode: mode_str.to_string(),
                    text: text.to_string(),
                },
            });
            id
        };

        // cc 忽略 task_id，随机 id 兼容 trait 签名
        let dummy_task = fuxi_core::id::TaskId::new();

        // 2. 若 interrupt：发 cancel；门客停 turn 后发 AgentInterrupted
        if interrupt_first {
            info!(agent = %agent_id, "intervene: 打断式");
            agent.cancel(dummy_task).await?;
            self.publish_with_agent(
                agent_id,
                EventKind::AgentInterrupted {
                    reason: "user_intervention".to_string(),
                },
            );
        } else {
            info!(agent = %agent_id, "intervene: 追加式");
        }

        // 3. 追加 user message（both modes 都走这步）
        agent.send_message(dummy_task, text).await?;

        // 4. TaskInterventionApplied —— wire 层确认
        self.publish_with_agent(
            agent_id,
            EventKind::TaskInterventionApplied {
                mode: mode_str.to_string(),
            },
        );

        // 5. 抄送（呈报）——target 非玄女且玄女 id 已设时，把副本发给玄女。
        // meta.agent 置为玄女，让订阅者知道"这条信归她知情"。
        // 公理 #2：玄女有知情权无否决权，不阻塞当前 intervene。
        let xuannv = self.xuannv_id().await;
        if let Some(xn) = xuannv
            && xn != agent_id
        {
            let mut meta = EventMeta::now();
            meta.agent = Some(xn);
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::OrchestratorCcReceived {
                    from_user_to: agent_id,
                    text: text.to_string(),
                    original_intervention_id: intervention_ev_id,
                },
            });
        }
        Ok(())
    }

    /// 把 task 置为 Blocked——玄女请示用户前发。v0.1 只发事件，**不动**
    /// orchestrator 的 shelf/运行时状态（cc 门客自己停在等待 user input 状态）。
    /// 事件是玄女和 Firehose 之间的"请示已就位"信号。
    ///
    /// 薄片 F 的 wire 层。v0.1 scenario spec 断言点 13。
    pub fn block_task(&self, task_id: fuxi_core::id::TaskId, reason: String) -> Result<()> {
        let mut meta = EventMeta::now();
        meta.task = Some(task_id);
        let _ = self.bus.publish(Event {
            meta,
            kind: EventKind::TaskBlocked { reason },
        });
        Ok(())
    }

    /// 解除 Blocked——玄女拿到授权后发。`input` 可选（"同意"/"同意，但改 X"/空 等）。
    ///
    /// v0.1 scenario spec 断言点 24。配合 `block_task` 完成"请示-授权"小循环。
    pub fn resume_task(&self, task_id: fuxi_core::id::TaskId, input: Option<String>) -> Result<()> {
        let mut meta = EventMeta::now();
        meta.task = Some(task_id);
        let _ = self.bus.publish(Event {
            meta,
            kind: EventKind::TaskResumed { input },
        });
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

/// 后台任务：订阅 bus 中的 `AgentDead` 事件，把对应 shelf 条目翻 Dead。
///
/// WHY 单独拆：让 Fuxi::with_config 构造期即可启动——构造函数不能 .await，所以这里
/// 仅做同步 `bus.subscribe()`（拿 broadcast Receiver 是同步操作）+ `tokio::spawn`。
/// shelf 被 Arc 共享：watcher 只持弱所有权也行，但 Arc 足够简单、无循环依赖。
fn spawn_death_watcher(bus: EventBus, shelf: Arc<Shelf>) {
    let mut sub = bus.subscribe();
    tokio::spawn(async move {
        while let Some(item) = sub.next().await {
            let Ok(ev) = item else {
                continue;
            };
            if let EventKind::AgentDead { .. } = ev.kind
                && let Some(id) = ev.meta.agent
            {
                shelf.set_status(id, ShelfStatus::Dead).await;
            }
        }
        debug!("death_watcher: bus 订阅流结束，退出");
    });
}
