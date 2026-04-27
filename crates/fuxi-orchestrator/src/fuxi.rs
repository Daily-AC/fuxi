//! `Fuxi`——玄女的主要入口。
//!
//! 生命周期：
//! 1. `Fuxi::new(bus, workspace)` 零门客启动。
//! 2. `spawn_worker(profile, WorkerKind::Cc(cfg))` 拉起具体门客，返回 `AgentId`。
//! 3. `dispatch(id, task)` 把 task 丢给指定门客——事件自动 republish 到 bus。
//! 4. `dispatch_to_any(role, task)` 是 **legacy 兼容壳**（内部转 task-bound）；
//!    新代码应直接使用 task-bound API：
//!    `dispatch_to_any_in_task(role, task_id, ...)` / `dispatch_in_task(...)`。
//! 5. `shutdown()` 关停所有门客进程；**不**销毁 worktree（保留供 P2 召回，
//!    见 Decision 07）——物理清理留给 `fuxi worktree clean`（v1.2）。
//!
//! 所有 mutating 方法是 `&self` 而非 `&mut self`——内部用 Arc+RwLock/Mutex。
//! 这样 `Arc<Fuxi>` 可以被多个后台 task 安全共享（CLI 的 REPL、A2A server 的
//! handler、世界模型 watcher 会一起持它）。

use crate::error::{OrchestratorError, Result};
use crate::recall::RecallSink;
use crate::registry::{Shelf, ShelfEntry, ShelfStatus};
use futures_util::StreamExt;
use fuxi_agent_cc::{CcAgent, CcLaunchConfig};
use fuxi_agent_codex::{CodexAgent, CodexLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::Task;
use fuxi_core::workspace::Workspace;
use fuxi_events::EventBus;
use fuxi_workspace::GitWorktreeWorkspace;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, watch};
use tracing::{debug, info, warn};

// turn 终态后给 pending-drain 新事件的宽限窗口。过大体感会慢，过小会丢尾包。
// 默认 50ms，必要时可用 FUXI_TERMINAL_DRAIN_GRACE_MS 覆盖。
const TERMINAL_DRAIN_GRACE_MS_DEFAULT: u64 = 50;

fn terminal_drain_grace_ms() -> u64 {
    std::env::var("FUXI_TERMINAL_DRAIN_GRACE_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(TERMINAL_DRAIN_GRACE_MS_DEFAULT)
}

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

/// 支持 spawn 的门客种类。gemini/opencode 随其适配器完成后加分支即可。
#[derive(Debug, Clone)]
pub enum WorkerKind {
    /// Claude Code 门客，带启动参数。
    Cc(CcLaunchConfig),
    /// OpenAI Codex CLI 门客（`codex exec --json`，spawn-per-dispatch）。
    Codex(CodexLaunchConfig),
}

impl WorkerKind {
    /// 对应到 `AgentProfile.cli` / `AgentSpawning.cli` 的文本标签。
    ///
    /// WHY 独立方法：集中在此避免每个调用点都 match 一次 `to_string`。
    /// 标签必须和 `fuxi-skills` loader 里 frontmatter `metadata.cli` 的取值
    /// 对齐——daemon::spawn_by_role 据此选 WorkerKind 分支。
    pub fn cli_tag(&self) -> &'static str {
        match self {
            WorkerKind::Cc(_) => "claude-code",
            WorkerKind::Codex(_) => "codex",
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
    ///
    /// `watch::Sender` 替代 `RwLock`（#7 修，公理 #3 真实时不轮询）：
    /// 调用方拿 [`Self::xuannv_id_watch`] 订阅 → 直接 `.changed().await`，
    /// 不需 5min 轮询。读路径仍走 `borrow()`，与原 `RwLock::read` 等价。
    xuannv_id: watch::Sender<Option<AgentId>>,
    /// P2 召回入库钩子。Why `Option`：默认 None 向后兼容——未设 sink 时
    /// dispatch pump silent skip，不阻塞 Done 流程。具体 impl 由 fuxi-cli 注入
    /// （参见 fuxi-cli/src/extractor_hook.rs 的反向依赖 pattern）。
    recall_sink: Arc<RwLock<Option<Arc<dyn RecallSink>>>>,
    /// β · #57 dispatch routing 钩子——dispatch 决策树命中 dist 路径
    /// （`task.pinned_node.is_some()` 或 `!task.required_tags.is_empty()`）时
    /// 调本钩子把 task 派给 dist controller。`Option`：未注入 = 不路由，所有
    /// dispatch 仍走本地 spawn（向后兼容 + 测试场景）。
    /// 同 RecallSink 反向依赖 pattern：trait 在本 crate，impl 由 fuxi-cli 注入。
    dist_enqueuer: Arc<RwLock<Option<Arc<dyn crate::DistEnqueuer>>>>,
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
        // watch::channel 初值 None——和原 `RwLock::new(None)` 等价的"未设置"态。
        // 接收端通过 `borrow()` 读当前值、`changed().await` 等下次 set。
        let (xuannv_tx, _) = watch::channel(None);
        let me = Self {
            bus: bus.clone(),
            workspace,
            shelf: shelf.clone(),
            cfg,
            xuannv_id: xuannv_tx,
            recall_sink: Arc::new(RwLock::new(None)),
            dist_enqueuer: Arc::new(RwLock::new(None)),
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
    /// 幂等：再次调用以最新值为准。订阅方 ([`Self::xuannv_id_watch`]) 会收到
    /// `changed()` 通知。
    pub async fn set_xuannv(&self, id: AgentId) {
        // send_replace：无 receiver 也不 panic（Fuxi 启动早于 IM 订阅时也安全）
        let _ = self.xuannv_id.send_replace(Some(id));
    }

    /// 读玄女 id——未设置返回 None。
    pub async fn xuannv_id(&self) -> Option<AgentId> {
        *self.xuannv_id.borrow()
    }

    /// 订阅玄女 id 变化——`#7` 公理 #3 真实时入口，替代旧 5min 轮询。
    ///
    /// 用法：
    /// ```ignore
    /// let mut rx = fuxi.xuannv_id_watch();
    /// // 已就绪 → 立即 borrow 拿值；否则 .changed().await 等下次 set
    /// if let Some(id) = *rx.borrow_and_update() { return id; }
    /// while rx.changed().await.is_ok() {
    ///     if let Some(id) = *rx.borrow_and_update() { return id; }
    /// }
    /// ```
    pub fn xuannv_id_watch(&self) -> watch::Receiver<Option<AgentId>> {
        self.xuannv_id.subscribe()
    }

    /// 注入 P2 召回入库钩子。fuxi-cli 启动时调一次；未调时 dispatch pump silent skip。
    /// 幂等：再次调用以最新值为准（测试场景偶尔会换 sink）。
    pub async fn set_recall_sink(&self, sink: Arc<dyn RecallSink>) {
        *self.recall_sink.write().await = Some(sink);
    }

    /// β · #57 注入 dispatch routing 钩子——dispatch 决策树命中 dist 路径
    /// （task.pinned_node 或 task.required_tags 非空）时调它把 task 派给 dist
    /// controller。
    /// 幂等：再次调用以最新值为准。生产由 fuxi-cli 在 `fuxi im start` 注入。
    pub async fn set_dist_enqueuer(&self, enqueuer: Arc<dyn crate::DistEnqueuer>) {
        *self.dist_enqueuer.write().await = Some(enqueuer);
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
        // 1. AgentSpawning + 2. worktree 分配（可能 None）
        self.publish_with_agent(
            agent_id,
            EventKind::AgentSpawning {
                role: profile.role.clone(),
                cli: kind.cli_tag().to_string(),
            },
        );
        let worktree = if self.cfg.allocate_worktree {
            Some(
                self.workspace
                    .create(agent_id, &self.cfg.base_branch)
                    .await?,
            )
        } else {
            None
        };
        info!(agent = %agent_id, role = %profile.role, "spawn worker");
        self.launch_and_register(agent_id, profile, kind, worktree)
            .await
    }

    /// P2 召回入口：复用一个已存在的 worktree path 起新门客。
    ///
    /// 和 `spawn_worker` 关键差别：**不调** `workspace.create`，把外部传入的 path 包
    /// 成 `borrowed: true` 的 `WorkspaceHandle`。该 handle 在 destroy 时不动 git
    /// （见 `WorkspaceHandle.borrowed`），让 worktree 留作下次召回。
    ///
    /// 用户通过 `fuxi spawn --recall-task/--recall-role` 触发；daemon 从 oracle
    /// 拿 worktree path 后调本方法。如果 path 在磁盘上不存在（被手动 rm 或 git
    /// worktree prune 了）— 不预检：cc launch 自己会以 cwd-not-exist 报错；caller
    /// 看到 launch 失败再决定 fallback 普通 spawn。
    pub async fn spawn_worker_in_worktree(
        &self,
        profile: AgentProfile,
        kind: WorkerKind,
        worktree_path: std::path::PathBuf,
        branch_hint: String,
    ) -> Result<AgentId> {
        let agent_id = AgentId::new();
        self.publish_with_agent(
            agent_id,
            EventKind::AgentSpawning {
                role: profile.role.clone(),
                cli: kind.cli_tag().to_string(),
            },
        );
        // 借用 handle——destroy 走 borrowed 短路，git worktree 不动。
        let handle = fuxi_core::workspace::WorkspaceHandle {
            agent: agent_id,
            repo_root: PathBuf::new(), // borrowed 不需要——destroy 看 borrowed=true 直接返
            worktree_path,
            branch: branch_hint,
            borrowed: true,
        };
        info!(
            agent = %agent_id,
            role = %profile.role,
            wt = %handle.worktree_path.display(),
            "spawn worker in borrowed worktree (recall)"
        );
        self.launch_and_register(agent_id, profile, kind, Some(handle))
            .await
    }

    /// `spawn_worker` / `spawn_worker_in_worktree` 共享的"已有 agent_id + 可选
    /// worktree → 跑 adapter launch → 注册 / 回滚"段。
    async fn launch_and_register(
        &self,
        agent_id: AgentId,
        mut profile: AgentProfile,
        kind: WorkerKind,
        worktree: Option<fuxi_core::workspace::WorkspaceHandle>,
    ) -> Result<AgentId> {
        // #48 决策 13 sentinel 教学注入——非黑名单 role + 未全局 disable 时，
        // 把 sentinel 用法写进 worker 的 system prompt addendum。
        // 详见 `crate::sentinel_addendum` 的 module doc。
        // cc 走 cc_cfg.append_system_prompt（cc 不读 profile.system_prompt）；
        // codex 走 profile.system_prompt（compose_prompt 在 dispatch 时 prepend）。
        // 故注入点跟分支一对一耦合，下面在 match 内各做一次。
        let inject_addendum = !crate::sentinel_addendum::is_globally_disabled()
            && crate::sentinel_addendum::should_inject_for_role(&profile.role);
        // β · #57 玄女专属 dispatch routing 教学——只 xuannv 注入，独立于 sentinel
        // 注入开关（routing 是派活契约，不归 sentinel 全局逃生口管）。
        let inject_routing =
            crate::sentinel_addendum::should_inject_routing_for_role(&profile.role);

        // 适配器 launch。每个分支都返回一个统一的
        //    `Result<(Arc<dyn Agent>, String /* endpoint_hint */), CoreError>`，
        //    后面共享同一段 register / 失败回滚逻辑。
        //    cc 还需要把 `take_death_watch` 的 rx 起转发——只有 cc 有 WS 死亡通道，
        //    codex 是 spawn-per-dispatch，进程在 dispatch 结束就退出，无需独立死亡 watcher。
        let launch_result: Result<(Arc<dyn Agent>, String)> = match kind {
            WorkerKind::Cc(mut cc_cfg) => {
                if let (None, Some(h)) = (cc_cfg.cwd.as_ref(), worktree.as_ref()) {
                    cc_cfg.cwd = Some(h.worktree_path.clone());
                }
                if inject_addendum {
                    // cc 专用：把 sentinel 教学拼到 --append-system-prompt
                    crate::sentinel_addendum::inject_cc(&mut cc_cfg);
                }
                if inject_routing {
                    // β · #57 玄女专属：派活路由规则注入（独立于 sentinel）
                    crate::sentinel_addendum::inject_xuannv_routing_cc(&mut cc_cfg);
                }
                match CcAgent::launch_with_id(agent_id, profile.clone(), cc_cfg).await {
                    Ok(a) => {
                        let endpoint_hint = a.card().endpoint.clone();
                        // 取出死亡信号接收端 → spawn 转发任务 → 死亡时 publish AgentDead。
                        // 放在 Arc::new 之前——take_death_watch 是 `&CcAgent` 方法，
                        // 装进 Arc<dyn Agent> 后就拿不动了。
                        if let Some(mut rx) = a.take_death_watch() {
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
                        Ok((Arc::new(a) as Arc<dyn Agent>, endpoint_hint))
                    }
                    Err(e) => Err(e.into()),
                }
            }
            WorkerKind::Codex(mut codex_cfg) => {
                if let (None, Some(h)) = (codex_cfg.cwd.as_ref(), worktree.as_ref()) {
                    codex_cfg.cwd = Some(h.worktree_path.clone());
                }
                if inject_addendum {
                    // codex 专用：把 sentinel 教学拼到 profile.system_prompt 末尾
                    crate::sentinel_addendum::inject_codex_profile(&mut profile);
                }
                match CodexAgent::launch_with_id(agent_id, profile.clone(), codex_cfg).await {
                    Ok(a) => {
                        let endpoint_hint = a.card().endpoint.clone();
                        Ok((Arc::new(a) as Arc<dyn Agent>, endpoint_hint))
                    }
                    Err(e) => Err(e.into()),
                }
            }
        };

        match launch_result {
            Ok((agent, endpoint_hint)) => {
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
                let cause = format!("launch failed: {e}");
                self.publish_with_agent(agent_id, EventKind::AgentDead { cause });
                Err(e)
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
        // β · #57 routing 决策树（spec gap e）——pinned_node / required_tags 非空 →
        // 走 dist enqueue（远端 worker 跑），否则继续本地 spawn / 已有 agent 路径。
        //
        // 已知缺口（v1）：
        // - dist 路径仍**先验证 agent_id 在 shelf 里**——保留这步是为了 dispatch
        //   契约一致（caller 传 agent_id 就该是个真 agent；玄女 dispatch 时给的
        //   是某个 placeholder 鲁班 id 即可）。后续 v1.x 可加纯 `dispatch_to_dist`
        //   入口允许 agent_id=None
        // - dist 路径不发 TaskCreated/TaskDispatched 事件到本进程 EventBus——
        //   dist worker 自己 emit 后通过 dist /dist/event publish 流回，本进程
        //   bus 自然能看到（共享 bus，#54 装配）
        let needs_dist = task.pinned_node.is_some() || !task.required_tags.is_empty();
        if needs_dist {
            let enqueuer_opt = self.dist_enqueuer.read().await.clone();
            if let Some(enqueuer) = enqueuer_opt {
                info!(
                    task_id = %task.id,
                    pinned_node = ?task.pinned_node,
                    required_tags = ?task.required_tags,
                    "dispatch routing: 走 dist enqueue（spec gap e）"
                );
                let _job_id = enqueuer.enqueue(&task).await?;
                return Ok(());
            }
            // enqueuer 未注入但 task 要 dist——降级到本地 spawn + warn
            // （生产 fuxi im start 必注入；走到这里一般是测试 / dev）
            warn!(
                task_id = %task.id,
                pinned_node = ?task.pinned_node,
                required_tags = ?task.required_tags,
                "dispatch routing: dist 路径但 enqueuer 未注入，fallback 本地 spawn"
            );
        }

        let agent = self
            .shelf
            .get_agent(agent_id)
            .await
            .ok_or(OrchestratorError::AgentNotFound(agent_id))?;

        // 派活开场事件：TaskCreated + TaskDispatched。
        // 历史 bug（2026-04-20 用户复测）：cc/codex adapter 都不主动发这两条，
        // 只有 agent 运行中的增量事件走 rx。结果 TUI 里门客永远卡在"空闲门客"
        // 桶——`upsert_task` 不会被触发。这里补上让 TUI / 观察器知道
        // 「task X 派给了 agent Y」。
        let task_id = task.id;
        let title = task.title.clone();
        let description = task.description.clone();
        {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent_id);
            meta.task = Some(task_id);
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::TaskCreated { title, description },
            });
        }
        {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent_id);
            meta.task = Some(task_id);
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::TaskDispatched { to: agent_id },
            });
        }

        self.shelf.set_status(agent_id, ShelfStatus::Busy).await;

        let mut rx = agent.dispatch(task).await?;
        let bus = self.bus.clone();
        let shelf = self.shelf.clone();
        // P2 召回：把 sink 和 agent 引用 clone 进 pump——Done 时 best-effort 入库。
        // why clone agent：session_id() 是 trait method，pump 内部需要直接调；
        // sink 取 snapshot（拿当下 setter 设的那个，不持锁等更新——pump 短命）。
        let recall_sink = self.recall_sink.read().await.clone();
        let recall_agent = agent.clone();
        tokio::spawn(async move {
            // M2.1+ 修 pending drain 漏洞（2026-04-20 用户复测发现）：
            // 旧逻辑看到 terminal 事件立即 break，但 agent pump 的 pending queue
            // drain 发生在 terminal 之**后**——drain 后 cc 会起新 turn 响应，
            // 那些事件需要继续走 rx→bus。break 早了 rx drop，pending drain 的
            // 新响应无 receiver。
            //
            // 新逻辑：terminal 后不立即 break，用短暂 grace timeout 等新事件；
            // 超时仍无 → 真 idle 退。这给 agent pump drain 一个窗口触发新 turn。
            let mut saw_terminal = false;
            let drain_grace_ms = terminal_drain_grace_ms();
            loop {
                let ev_opt = if saw_terminal {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(drain_grace_ms),
                        rx.recv(),
                    )
                    .await
                    {
                        Ok(Some(ev)) => Some(ev),
                        Ok(None) => None, // rx 被 agent 关闭
                        Err(_) => {
                            // terminal 后 grace 窗口内无新事件——agent 真 idle
                            break;
                        }
                    }
                } else {
                    rx.recv().await
                };
                let Some(ev) = ev_opt else { break };

                // P2 召回入库——仅 Done（不是任意 terminal）。
                // why 仅 Done：Cancelled/Dead 是失败终结，session 可能没意义甚至有脏 context；
                // 召回基于"完成态"避免拉出半截数据。Blocked/Delivered 不是终结所以也跳。
                let is_done = matches!(
                    &ev.kind,
                    EventKind::TaskStateChanged {
                        to: fuxi_core::task::TaskState::Done,
                        ..
                    }
                );
                if is_done && let Some(sink) = recall_sink.as_ref() {
                    // 收齐 RecallContext 整包传 sink——pump 不再判 session_id 是否 None
                    // （codex 永远 None 但 worktree 有）；sink 自行决定写哪些 fact。
                    let role = recall_agent.card().profile.role.clone();
                    let worktree = shelf.worktree_of(agent_id).await;
                    let cli_session_id = recall_agent.session_id().await;
                    sink.record(crate::recall::RecallContext {
                        agent_id,
                        task_id,
                        role,
                        worktree,
                        cli_session_id,
                    })
                    .await;
                }

                // WHY：dispatch turn 终态视角看以下三类——
                //   1. `TaskStateChanged{Done|Cancelled}`：cc/codex 干完
                //   2. `AgentDead`：cc/codex 进程崩溃
                //   3. `TaskBlocked`：cc/codex 自身把当前 turn 打到 Blocked
                //      （cc `ResultError` / codex `TurnFailed` 都映射到此），
                //      cc 内部已进 Idle 等用户干预——dispatch 这单不会再出新事件
                //
                // M3.6 删掉 TaskDelivered/TaskCancelled 孤儿后不再兜底。
                // #19 修：之前 `TaskBlocked` 不在终态——cc/codex 报错时 ws_pump 进
                // 内部 Idle，但 Fuxi pump 永远等不到 Done，shelf 锁死 Busy。
                // 治本：Blocked 也算 turn 结束（task 本身仍是 Blocked 可恢复态，
                // 等 `resume_task` 触发新 dispatch 即可——pump 寿命 ≤ 单 turn）。
                let is_terminal = matches!(
                    &ev.kind,
                    EventKind::TaskStateChanged {
                        to: fuxi_core::task::TaskState::Done
                            | fuxi_core::task::TaskState::Cancelled,
                        ..
                    } | EventKind::AgentDead { .. }
                        | EventKind::TaskBlocked { .. }
                );
                if bus.publish(ev).is_err() {
                    warn!(agent = %agent_id, "event bus 已关闭，pump 退出");
                    break;
                }
                if is_terminal {
                    saw_terminal = true;
                } else if saw_terminal {
                    // terminal 后收到新事件 = drain 的新 turn 启动了；重置等再次 terminal
                    saw_terminal = false;
                }
            }
            // pump 退出默认摊回 Idle，但若已被 death_watcher 标 Dead（AgentDead），
            // 不能回写成 Idle——否则会出现"门客死亡后又可用"的状态回退。
            // #19 增 info 级日志：用户复现"门客 Idle 但 task 无收尾"时，journal 可以
            // 一眼看到 pump 在哪个分支退出（terminal 见到 vs 没见到 vs bus 关）。
            let prev_status = shelf.status_of(agent_id).await;
            if prev_status != Some(ShelfStatus::Dead) {
                shelf.set_status(agent_id, ShelfStatus::Idle).await;
            }
            info!(
                agent = %agent_id,
                task = %task_id,
                saw_terminal,
                prev_status = ?prev_status,
                "dispatch pump 退出"
            );
        });

        Ok(())
    }

    /// 给指定门客派活，但复用一个已有 task_id（父任务 fan-out 场景）。
    ///
    /// 用法：先拿到一个父任务 id，再把同 id 派给多个门客。事件流里这些门客会共享
    /// 同一个 `meta.task`，TUI 可按 task-rooted 聚合。
    pub async fn dispatch_in_task(
        &self,
        agent_id: AgentId,
        task_id: TaskId,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<()> {
        let mut task = Task::new(title, description);
        task.id = task_id;
        self.dispatch(agent_id, task).await
    }

    /// 介入——向某个门客发话。
    ///
    /// - `append`：追加一条 user message，门客下一 turn 看到（stdio/WS 都能做）
    /// - `interrupt`：打断当前 turn 再追加（依赖 WS 模式的 control_request/interrupt）
    ///
    /// v0.1 薄片 I 承诺的三个事件：
    /// - `UserInterventionSent { target, mode, text, mentions }`  （入口）
    /// - `AgentInterrupted { reason }`   仅在 interrupt 模式下发
    /// - `TaskInterventionApplied { mode }`  wire 层确认
    ///
    /// `mentions`（v3 #N7'）：用户消息里所有被 @ 的 agent_id，前端约定含
    /// `target` 自身。后端不强制语义检查（前端保证），仅写入事件用作历史回放
    /// 时还原 chip 视觉。空 Vec = 无 @（对应 v0.1 旧入口、TUI、内部 degrade）。
    ///
    /// `pinned_node`（β · #57）：用户在 PWA composer 用 `@<node_id>` 显式
    /// pin 到的 dist 节点（如 `mac-local`）。**v1 范围内仅写入事件供 audit /
    /// 历史回放使用**——真路由要走 `Fuxi::dispatch` 决策树（task.pinned_node /
    /// task.required_tags），intervene 路径暂不直接派 dist enqueue。
    /// **已知缺口**：intervene busy worker 时 send_message 仍走本地 agent；
    /// `pinned_node` 在该路径暂忽略。idle 退化 dispatch 路径会通过 task.pinned_node
    /// 把它真路由（spec gap e v1）。
    ///
    /// cc 适配器忽略 task_id，这里传随机 id 兼容 trait 签名；事件上不挂
    /// task 维度（没有从 dispatch 回流最近 task 的路径）——v0.2 补上"最近
    /// dispatch 的 task 记忆"后再加。
    pub async fn intervene(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
        mentions: Vec<AgentId>,
        pinned_node: Option<String>,
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
                        mentions: mentions.clone(),
                        pinned_node: pinned_node.clone(),
                    },
                });
                id
            };
            let _ = agent; // 不再直接操作 agent，下面 dispatch 内部会再拿一次
            // 2026-04-20 改：title 从 "intervention" → "user-turn"——
            // 语义上就是一轮用户对话，和 TUI Submit::Xuannv 统一，避免混两种 task 类型
            //
            // β · #57：把 intervene 的 pinned_node 写到 task 上，让下面
            // self.dispatch 决策树命中 dist 路径派远端节点。required_tags v1
            // 暂不从 intervene 入口传（玄女自己派活时填，PWA composer 仅显式
            // pinned_node 一级 routing）。
            let mut task = Task::new("user-turn", text);
            if let Some(node) = pinned_node.clone() {
                task = task.with_pinned_node(node);
            }
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
                    mentions,
                    pinned_node,
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

    /// legacy 兼容壳：保留旧签名，但内部统一转到 task-bound 语义。
    ///
    /// WHY：避免新旧派工语义并存导致的认知分叉（idle 复用 vs task 归属）。
    /// 旧调用方不改签名也能跑，但行为与 `dispatch_to_any_in_task` 对齐。
    pub async fn dispatch_to_any(
        &self,
        role: &str,
        task: Task,
        profile_template: AgentProfile,
        kind_for_spawn: WorkerKind,
    ) -> Result<AgentId> {
        warn!(
            role = %role,
            task = %task.id,
            "dispatch_to_any: legacy 兼容壳（内部转 task-bound）；建议迁移到 dispatch_to_any_in_task/dispatch_in_task"
        );
        self.dispatch_to_any_in_task(
            role,
            task.id,
            task.title,
            task.description,
            profile_template,
            kind_for_spawn,
        )
        .await
    }

    /// `dispatch_to_any` 的 task-bound 版本：**不复用 idle**，而是显式 spawn 一个
    /// 新门客，再把它绑定到同一个父 task_id。
    ///
    /// 这条路径是“严格 task-bound 派工”：适合一个 task 下并行派出多个门客的
    /// 场景，语义上和 `dispatch_to_any` 分开，避免旧 idle 语义污染 task 归属。
    pub async fn dispatch_to_any_in_task(
        &self,
        role: &str,
        task_id: TaskId,
        title: impl Into<String>,
        description: impl Into<String>,
        profile_template: AgentProfile,
        kind_for_spawn: WorkerKind,
    ) -> Result<AgentId> {
        let mut p = profile_template;
        p.role = role.to_string();
        let chosen = self.spawn_worker(p, kind_for_spawn).await?;
        self.dispatch_in_task(chosen, task_id, title, description)
            .await?;
        Ok(chosen)
    }

    /// 停掉单个门客——M2.4 idle GC 的落地钩子。
    ///
    /// 语义与 `shutdown()` 对齐：事件顺序 `AgentShuttingDown`（reason 自带）→
    /// agent.shutdown + worktree.destroy → `AgentDead`；worktree/agent 清理出错只
    /// warn 不传播，避免单只门客回收失败阻塞整个 GC tick。
    ///
    /// 幂等：id 找不到（已被清走）返回 Ok(())；`fuxi kill --id` 留给 M3.7。
    ///
    /// **玄女豁免**：shutdown_agent 拒绝杀玄女本人——她是用户对话唯一入口，
    /// 被 kill 整个 TUI 崩。只有 `Fuxi::shutdown()`（平台整体下线）能碰她。
    /// GC / 将来的 `fuxi kill --id` 都走这个豁免。
    pub async fn shutdown_agent(&self, id: AgentId, reason: String) -> Result<()> {
        if let Some(xn) = self.xuannv_id().await
            && xn == id
        {
            warn!(
                agent = %id,
                reason = %reason,
                "shutdown_agent: 拒绝杀玄女（豁免）——平台整体 shutdown 才能关玄女"
            );
            return Ok(());
        }
        let Some(entry) = self.shelf.take(id).await else {
            // 已被清走（并发 GC / 手动 shutdown）——noop，外层不用特判。
            debug!(agent = %id, "shutdown_agent: 门客不在 shelf，跳过");
            return Ok(());
        };
        info!(agent = %id, reason = %reason, "shutdown_agent");
        self.publish_with_agent(
            id,
            EventKind::AgentShuttingDown {
                reason: reason.clone(),
            },
        );
        if let Err(e) = entry.agent.shutdown().await {
            warn!(agent = %id, error = %e, "agent shutdown 出错");
        }
        // P2 召回边界（Decision 07）：shutdown 默认**不销毁 worktree**——留作召回 stash。
        // 用户重开 fuxi 后 `--recall-task/role` 才能复用旧 cwd，cc session 文件也才在。
        // 物理清理由专门的 `fuxi worktree clean`（v1.2）做；borrowed handle 本就 noop。
        if let Some(h) = entry.worktree.as_ref() {
            tracing::debug!(
                agent = %id,
                wt = %h.worktree_path.display(),
                "shutdown_agent: 保留 worktree 供召回 stash"
            );
        }
        self.publish_with_agent(id, EventKind::AgentDead { cause: reason });
        Ok(())
    }

    /// 停掉所有门客（仅 stop process，不动 worktree）。幂等。
    ///
    /// 事件顺序（每个门客）：`AgentShuttingDown` → agent.shutdown → `AgentDead`。
    /// **不**销毁 worktree——P2 召回（Decision 07）要求 worktree 跨 daemon 重启可用，
    /// 物理清理由 `fuxi worktree clean`（v1.2）显式做。
    pub async fn shutdown(&self) -> Result<()> {
        let cards = self.shelf.list_cards().await;
        info!(
            count = cards.len(),
            "fuxi shutdown: 关闭所有门客（保留 worktree 供召回）"
        );
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
                // 新 spawn 的门客立即算作"刚进入 idle"——TTL 从这一刻起计时。
                idle_since: Some(std::time::Instant::now()),
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

/// `Fuxi` 实现 [`crate::idle_gc::IdleShutdowner`]——GC 任务拿 `Arc<Fuxi>` 通过
/// `Arc<dyn IdleShutdowner>` 的 unsize coercion 直接调用。
///
/// WHY 不 impl 在 `Arc<Fuxi>` 上：那样 `Arc<Fuxi> → Arc<dyn Trait>` 的
/// `CoerceUnsized` 不成立（coercion 要求目标 `dyn Trait` 对 `Fuxi` 本身成立）。
#[async_trait::async_trait]
impl crate::idle_gc::IdleShutdowner for Fuxi {
    async fn shutdown_idle(&self, id: AgentId, reason: String) -> Result<()> {
        self.shutdown_agent(id, reason).await
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

#[cfg(test)]
mod worker_kind_tests {
    use super::*;
    use fuxi_agent_codex::CodexLaunchConfig;

    /// 守门：cli_tag 必须分别返回 cc / codex 适配器对应的标签。daemon::spawn_by_role
    /// 用 `profile.cli` 反查 WorkerKind 分支，标签飘了就 spawn 不出来。
    #[test]
    fn cli_tag_distinguishes_cc_and_codex() {
        let cc = WorkerKind::Cc(CcLaunchConfig::default());
        let codex = WorkerKind::Codex(CodexLaunchConfig::default());
        assert_eq!(cc.cli_tag(), "claude-code");
        assert_eq!(codex.cli_tag(), "codex");
    }
}
