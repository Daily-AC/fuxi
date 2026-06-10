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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    /// Decision 21 phase 1：注册 + 找 Project；持有了之后 `spawn_worker_in_project_sandbox`
    /// 才能 lookup project canonical_path 然后驱动 PersistentSandboxManager。
    /// `Option`：未注入 = 走旧 agent-id worktree 路径（向后兼容）。
    project_registry: Arc<RwLock<Option<Arc<fuxi_workspace::FileSystemProjectRegistry>>>>,
    /// Decision 21 phase 3 磁盘 quota 缓存——`(measured_at, total_bytes)`。
    /// 每次 spawn 检查时若距上次 < TTL 用缓存值，否则递归扫描 `<projects_root>/<project>/`。
    /// **避免每次 spawn 全量扫多 GB sandbox 拖慢启动**。
    disk_quota_cache: Arc<RwLock<HashMap<fuxi_core::ProjectId, (std::time::Instant, u64)>>>,
    /// memory-v2 (#48) 注入桥用的 store 句柄——`launch_and_register` 在 spawn worker
    /// 时拉用户身份卡 + 同 role 历史心法，拼到 cc/codex 的 system prompt addendum。
    /// `Option`：未注入 = 完全跳过 memory 注入（向后兼容；测试时也方便不带 store 跑）。
    /// 由 fuxi-cli `fuxi im start` 在启动期注入同一份 SQLite pool 的 store。
    memory_stores: Arc<RwLock<Option<MemoryStores>>>,
    /// v2 跨节点 sandbox · 节点负载数据源——dispatch 在 task 关联到 project 但
    /// 未显式 pin 时，调本 provider 拿当前各节点 inflight/max_concurrency 选最闲。
    /// `Option`：未注入 = auto-pin 路径 short-circuit 返 None（fallback 本地路径）。
    /// 同 DistEnqueuer pattern：production impl 由 fuxi-cli 包 `Arc<DistController>` 提供。
    node_load_provider: Arc<RwLock<Option<Arc<dyn crate::NodeLoadProvider>>>>,
    /// Phase 1 topic 路由：当前玄女绑定的 topic_id。初值 [`TopicId::general()`]
    /// （Phase 1 之前唯一 topic）。`watch` 让 SystemEventBridge / conv_store sync /
    /// 桌面端 sidebar 都能 `.changed().await` 实时跟随，公理 #3 真实时不轮询。
    /// 由 fuxi-cli `topic_switch::switch_topic_to` 在 kill+spawn 新玄女后更新。
    current_topic_id: watch::Sender<fuxi_core::TopicId>,
    /// Phase 1 切 topic 反向依赖入口——fuxi-im 的 `/api/topics/:id/switch` 用它。
    /// trait 在 orchestrator（最小 vocab），impl 在 fuxi-cli `topic_switch` 包
    /// switch_topic_to 注入。`Option` 未注入 = handler 返 503（同 RecallSink pattern）。
    xuannv_switcher: Arc<RwLock<Option<Arc<dyn crate::XuannvSwitcher>>>>,
    /// 块2 玄女分身池：topic_id → 活分身 AgentId。替代单 `xuannv_id` 的多分身模型。
    /// `xuannv_id` watch 仍保留为 **general topic 分身的镜像**（兼容壳 + idle_gc
    /// 单豁免 fallback）：`set_xuannv_for_topic(general, id)` 会同步 push 到它。
    /// 上限由 `FUXI_XUANNV_MAX_ACTIVE` 注入（默认 3）。
    xuannv_pool: Arc<crate::xuannv_pool::XuannvPool>,
    /// 块4 持久队列钩子——bridge 在归属 topic 分身 dormant 时把完工/里程碑信号
    /// 落库（a01cfab5「信号不丢」），分身 respawn 后 drain 补发（块5 收口）。
    /// `Option`：未注入 = 单玄女兼容期 / 测试，`enqueue_pending` debug 跳过。
    /// trait 在本 crate，impl adapter 由 fuxi-cli 注入（依赖反转，见
    /// [`crate::PendingNotifySink`] doc）。
    pending_sink: Arc<RwLock<Option<Arc<dyn crate::PendingNotifySink>>>>,
    /// 块5 玄女分身懒启动钩子——`ensure_xuannv_for_topic` 池 miss 时调它为该 topic
    /// spawn 一只分身（拉历史 prelude + cc launch，逻辑在 fuxi-cli adapter）。
    /// `Option`：未注入 = 单玄女兼容期 / 测试，`ensure_xuannv_for_topic` 退化为
    /// 「池里有就返回，没有返 None」（不 spawn）。依赖反转见 [`crate::XuannvSpawner`]。
    xuannv_spawner: Arc<RwLock<Option<Arc<dyn crate::XuannvSpawner>>>>,
}

/// memory-v2 注入桥需要的两个 store 句柄。两者来自同一 SQLite 文件
/// （events.db）但不同 table，clone 便宜（内部 `Arc<SqlitePool>`）。
#[derive(Clone)]
pub struct MemoryStores {
    pub user_profile: fuxi_memory::UserProfileStore,
    pub hetu: fuxi_memory::HetuStore,
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
        // Phase 1：current_topic_id 初值 general。switch_topic 前所有玄女对话都
        // 落在 general topic（兼容老行为）。
        let (topic_tx, _) = watch::channel(fuxi_core::TopicId::general());
        // 块2：玄女分身池上限走 env，非法 / 缺省 = 3（同时活 3 个 topic 的分身，
        // 超出按 LRU dormant 回收）。
        let max_active = std::env::var("FUXI_XUANNV_MAX_ACTIVE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(3);
        let me = Self {
            bus: bus.clone(),
            workspace,
            shelf: shelf.clone(),
            cfg,
            xuannv_id: xuannv_tx,
            recall_sink: Arc::new(RwLock::new(None)),
            dist_enqueuer: Arc::new(RwLock::new(None)),
            project_registry: Arc::new(RwLock::new(None)),
            disk_quota_cache: Arc::new(RwLock::new(HashMap::new())),
            memory_stores: Arc::new(RwLock::new(None)),
            node_load_provider: Arc::new(RwLock::new(None)),
            current_topic_id: topic_tx,
            xuannv_switcher: Arc::new(RwLock::new(None)),
            xuannv_pool: Arc::new(crate::xuannv_pool::XuannvPool::new(max_active)),
            pending_sink: Arc::new(RwLock::new(None)),
            xuannv_spawner: Arc::new(RwLock::new(None)),
        };
        // 死亡检测：Fuxi 自订阅 bus，看到 AgentDead 即把对应 shelf 条目翻 Dead。
        // why 放在这里：唯一拥有 shelf 写权限的地方；具体死亡检测源头（cc WS 关闭、
        // Fuxi::shutdown 主动发、外部 publish）全部汇入这一条路径。
        spawn_death_watcher(bus, shelf);
        // 块5：general 镜像 reconciler——`xuannv_id` watch（兼容壳 + bridge general
        // fallback + conv_store sync 读它）必须始终跟随池里 general 分身。让池做唯一
        // 真相源：任何 mutator（set_xuannv_for_topic / handoff remove / **idle_gc
        // dormant remove**）改了 general 入口，reconciler 自动把镜像同步成最新值（含
        // 回收后置 None）。**修今天的黑洞坑**：dormant 回收 general 只 pool.remove 不清
        // 镜像 → xuannv_id() 返已死 id → 用户消息黑洞；reconciler 兜住这条。
        spawn_general_mirror_sync(me.xuannv_pool.watch(), me.xuannv_id.clone());
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
    ///
    /// 块2 兼容壳：等价于把分身绑到 [`TopicId::general`]——未迁移到 topic 维度的
    /// 调用方（repl 启动、handoff、抄送判定）继续走单玄女语义，落在 general topic。
    pub async fn set_xuannv(&self, id: AgentId) {
        self.set_xuannv_for_topic(fuxi_core::TopicId::general(), id)
            .await;
    }

    /// 读玄女 id——未设置返回 None。
    ///
    /// 块2 兼容壳：返回 general topic 的活分身（单玄女语义下的"那一个"玄女）。
    /// 抄送 / handoff / shutdown 单豁免 fallback 仍读这个。
    pub async fn xuannv_id(&self) -> Option<AgentId> {
        *self.xuannv_id.borrow()
    }

    /// 块2：绑定某 topic → 活分身（spawn / respawn 后调）。
    ///
    /// general topic 的绑定**同步**写一次 `xuannv_id` watch——保持兼容壳
    /// [`Self::xuannv_id`] + bridge general fallback 立即读到新值（零 lag，老测试
    /// 紧跟 `rx.changed()` 的语义不破）。general 的**移除**（dormant 回收 / handoff）
    /// 不走本方法，由 [`spawn_general_mirror_sync`] reconciler 兜（置 None）。两路对
    /// 同一值幂等。非 general topic 只进池。
    pub async fn set_xuannv_for_topic(&self, topic: fuxi_core::TopicId, id: AgentId) {
        self.xuannv_pool.set_active(topic, id).await;
        if topic == fuxi_core::TopicId::general() {
            // send_replace：无 receiver 也不 panic（Fuxi 启动早于 IM 订阅时也安全）
            let _ = self.xuannv_id.send_replace(Some(id));
        }
    }

    /// 块2：读某 topic 的活分身 id——无活分身（从未起 / 已 dormant）返回 None。
    pub async fn xuannv_id_for_topic(&self, topic: fuxi_core::TopicId) -> Option<AgentId> {
        self.xuannv_pool.active_id(topic).await
    }

    /// 块2：订阅 topic→分身全量映射的实时视图（跟随 respawn 漂移）。
    /// SystemEventBridge 块3 用它把里程碑事件路由到归属 topic 的分身。
    pub fn xuannv_pool_watch(
        &self,
    ) -> tokio::sync::watch::Receiver<HashMap<fuxi_core::TopicId, AgentId>> {
        self.xuannv_pool.watch()
    }

    /// 块2：克隆池 Arc——idle_gc 需要它做 dormant 回收（topic_of + remove）。
    pub fn xuannv_pool(&self) -> Arc<crate::xuannv_pool::XuannvPool> {
        self.xuannv_pool.clone()
    }

    /// 块5 懒启动入口：拿某 topic 的活分身；池有直接返回，**池 miss 则真 spawn**。
    ///
    /// - 池有活分身 → 返回它（热路径，零 spawn）。
    /// - 池 miss + 已注入 [`Self::set_xuannv_spawner`] → 调 spawner 为该 topic 起一只
    ///   （adapter 负责拉历史 prelude + cc launch + `set_xuannv_for_topic` 入池），
    ///   返回新 id。spawn 失败 → warn + 返回 None（调用方按需 fallback，不 panic）。
    /// - 池 miss + 未注入 spawner（单玄女兼容期 / 测试）→ 返回 None（退化为查池）。
    ///
    /// 返回类型保持 `Option<AgentId>`（兼容块3 调用方）：None = 当前确实没有可用分身。
    ///
    /// WHY 不在此加锁防并发双 spawn：lazy 入口（用户消息 / bridge respawn）实际串行
    /// 度高；真撞上 set_xuannv_for_topic 是 last-write-wins（同一 topic 最后那只赢，
    /// 多起的那只 idle 后被 GC 回收）。加跨 spawn 的锁会把慢 cc launch 串死整条入口，
    /// 得不偿失——同 spawn 语义「新建不去重」公理（fbba2ec）。
    pub async fn ensure_xuannv_for_topic(&self, topic: fuxi_core::TopicId) -> Option<AgentId> {
        if let Some(id) = self.xuannv_id_for_topic(topic).await {
            return Some(id);
        }
        let spawner = self.xuannv_spawner.read().await.clone();
        match spawner {
            Some(s) => match s.spawn_for_topic(topic).await {
                Ok(id) => Some(id),
                Err(e) => {
                    warn!(%topic, error = %e, "ensure_xuannv_for_topic: spawn 失败，返回 None");
                    None
                }
            },
            None => {
                debug!(%topic, "ensure_xuannv_for_topic: 未注入 spawner，池 miss 返回 None");
                None
            }
        }
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

    /// Phase 1：读当前玄女绑定的 topic。冷启动 / 未切过为 [`TopicId::general()`]。
    pub fn current_topic_id(&self) -> fuxi_core::TopicId {
        *self.current_topic_id.borrow()
    }

    /// Phase 1：订阅当前 topic 变化——SystemEventBridge / conv_store sync /
    /// PWA sidebar 都按此 receiver 实时跟随，避免轮询。
    pub fn current_topic_watch(&self) -> watch::Receiver<fuxi_core::TopicId> {
        self.current_topic_id.subscribe()
    }

    /// Phase 1：把当前 topic 切到 `id`。**只更新 watch**，不动 cc 进程——
    /// 真切 cc + 拉 prelude 由 fuxi-cli 的 `topic_switch::switch_topic_to`
    /// 做（它先 kill old + spawn new + 注 prelude，最后调本方法 commit）。
    /// 幂等：相同 id 重复 set 仍发 changed notification（订阅方按需去重）。
    pub async fn set_current_topic(&self, id: fuxi_core::TopicId) {
        let _ = self.current_topic_id.send_replace(id);
    }

    /// Phase 1：注入切玄女 topic 的反向依赖 impl（fuxi-cli `topic_switch` 包
    /// `switch_topic_to`）。fuxi-im handler 通过 [`Self::xuannv_switcher`] 拿
    /// trait object 调用，避免 fuxi-im 反向依赖 fuxi-cli。
    pub async fn set_xuannv_switcher(&self, switcher: Arc<dyn crate::XuannvSwitcher>) {
        *self.xuannv_switcher.write().await = Some(switcher);
    }

    /// Phase 1：拿当前 xuannv_switcher impl（None = fuxi-cli 启动期还未注入，
    /// handler 视作 503 Service Unavailable）。
    pub async fn xuannv_switcher(&self) -> Option<Arc<dyn crate::XuannvSwitcher>> {
        self.xuannv_switcher.read().await.clone()
    }

    /// 注入 P2 召回入库钩子。fuxi-cli 启动时调一次；未调时 dispatch pump silent skip。
    /// 幂等：再次调用以最新值为准（测试场景偶尔会换 sink）。
    pub async fn set_recall_sink(&self, sink: Arc<dyn RecallSink>) {
        *self.recall_sink.write().await = Some(sink);
    }

    /// 块4：注入持久队列钩子（dormant 分身的完工信号落库）。fuxi-cli 启动期注入
    /// 包 `PendingNotifyStore` 的 adapter；未注入时 [`Self::enqueue_pending`] debug
    /// 跳过（单玄女兼容期 / 测试）。幂等。
    pub async fn set_pending_sink(&self, sink: Arc<dyn crate::PendingNotifySink>) {
        *self.pending_sink.write().await = Some(sink);
    }

    /// 块4：读当前持久队列 sink（None = 未注入）。bridge 的 `Intervener::enqueue_pending`
    /// impl 用它转发；放 `pub(crate)` 不外泄 RwLock 细节。
    pub(crate) async fn pending_sink_handle(&self) -> Option<Arc<dyn crate::PendingNotifySink>> {
        self.pending_sink.read().await.clone()
    }

    /// 块5：注入玄女分身懒启动钩子（fuxi-cli adapter 复用 spawn_with_prelude +
    /// topic 历史）。未注入时 [`Self::ensure_xuannv_for_topic`] 不 spawn 退化为查池。
    /// 幂等。
    pub async fn set_xuannv_spawner(&self, spawner: Arc<dyn crate::XuannvSpawner>) {
        *self.xuannv_spawner.write().await = Some(spawner);
    }

    /// β · #57 注入 dispatch routing 钩子——dispatch 决策树命中 dist 路径
    /// （task.pinned_node 或 task.required_tags 非空）时调它把 task 派给 dist
    /// controller。
    /// 幂等：再次调用以最新值为准。生产由 fuxi-cli 在 `fuxi im start` 注入。
    pub async fn set_dist_enqueuer(&self, enqueuer: Arc<dyn crate::DistEnqueuer>) {
        *self.dist_enqueuer.write().await = Some(enqueuer);
    }

    /// Decision 21 phase 1：注入 ProjectRegistry。
    /// 注入后 `spawn_worker_in_project_sandbox` 才能用——否则该方法返
    /// `Other("project_registry 未注入")`。生产 `fuxi im start` 时由 caller 注入
    /// 同一 `FileSystemProjectRegistry::with_default_root()` 实例（PWA / CLI 跟
    /// orchestrator 共享一份注册表）。
    pub async fn set_project_registry(
        &self,
        registry: Arc<fuxi_workspace::FileSystemProjectRegistry>,
    ) {
        *self.project_registry.write().await = Some(registry);
    }

    /// memory-v2 (#48) 注入桥——把 user_profile + hetu 心法 store 绑给 Fuxi。
    /// 注入后 `launch_and_register` 在 spawn 每个 cc/codex 门客时会自动从这两表
    /// 拉身份卡 + 同 role 心法拼到 system prompt（黑名单 xuannv/extractor/cangjie 跳过）。
    /// 幂等；未注入时所有 spawn 跳过 memory 注入（向后兼容）。
    pub async fn set_memory_stores(&self, stores: MemoryStores) {
        *self.memory_stores.write().await = Some(stores);
    }

    /// v2 跨节点 sandbox：注入节点负载数据源。dispatch 在 task 关联到 project
    /// 但未显式 pin 时，调本 provider 拿快照按 saturation 选最闲节点。
    /// 幂等；未注入 = auto-pin short-circuit（保留 v1 行为）。
    pub async fn set_node_load_provider(&self, provider: Arc<dyn crate::NodeLoadProvider>) {
        *self.node_load_provider.write().await = Some(provider);
    }

    /// v2 跨节点 sandbox：根据 task.project_id 决定 auto-pin 节点。
    ///
    /// 行为：
    /// - task.pinned_node 已 Some → 返 None（用户意图优先，不覆盖）
    /// - task.project_id None → 返 None
    /// - registry/load_provider 任一未注入 → 返 None
    /// - project.host_nodes 空 → 返 None（v1 单节点项目，保留旧行为）
    /// - 候选全离线 → 返 None
    /// - 否则取候选中 saturation 最低的 node_id
    pub async fn auto_pin_from_project(&self, task: &Task) -> Option<String> {
        if task.pinned_node.is_some() {
            return None;
        }
        let project_id = task.project_id.as_ref()?;

        let registry = self.project_registry.read().await.clone()?;
        let project = registry.get(project_id).await.ok().flatten()?;
        if project.host_nodes.is_empty() {
            return None;
        }

        let provider = self.node_load_provider.read().await.clone()?;
        let snapshots = provider.snapshot().await;
        crate::node_load::pick_least_loaded(&snapshots, &project.host_nodes)
            .map(|s| s.node_id.clone())
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

    /// 原子地按 role 找一只 Idle 门客并标记为 Busy；找不到返 None。
    ///
    /// issue eebe38ef：cangjie spawner 之前走 `dispatch_to_any_in_task` 路径
    /// **不复用 idle**，每个 task done / batch judge 都净新增 2 只。暴露这条
    /// 公开 API 让 spawner adapter 自己判断"先认领 idle、否则 spawn 新的"。
    /// 普通 dispatch 路径仍走 task-bound 不复用（语义不变）。
    pub async fn claim_idle_by_role(&self, role: &str) -> Option<AgentId> {
        self.shelf.claim_idle_by_role(role).await
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

    /// Decision 21 phase 1：在已注册项目的 L3 持久 sandbox 里 spawn 门客。
    ///
    /// 跟 `spawn_worker_in_worktree` 关键差别：
    /// - worktree 路径不是 caller 算的，而是 `PersistentSandboxManager::get_or_create`
    ///   按 (project, role) 算的——同 role 的不同 task 共用 sandbox（保 build cache + WIP）
    /// - branch 是 `<role>/<project>-main` 长期 branch，不是 task-级 short ttl
    /// - WorkspaceHandle 仍标 `borrowed: true`，destroy 不动 git（sandbox 长期存活）
    /// - `project_registry` 必须先 `set_project_registry` 注入；未注入返 Other 错误
    ///
    /// **不**改老 `spawn_worker` 路径——本方法是 opt-in 入口。CLI / IM 后续若要
    /// 默认走 sandbox 模式，再单独切换调用方。
    pub async fn spawn_worker_in_project_sandbox(
        &self,
        mut profile: AgentProfile,
        kind: WorkerKind,
        project_id: fuxi_core::ProjectId,
        role_for_sandbox: String,
    ) -> Result<AgentId> {
        // 1. 拿 project registry + project meta
        let registry_opt = self.project_registry.read().await.clone();
        let registry = registry_opt.ok_or_else(|| {
            OrchestratorError::Other(
                "project_registry 未注入 —— 调 Fuxi::set_project_registry 后再用".into(),
            )
        })?;
        let project = registry
            .get(&project_id)
            .await
            .map_err(|e| OrchestratorError::Other(format!("project lookup 失败: {e}")))?
            .ok_or_else(|| OrchestratorError::Other(format!("project {project_id} 未注册")))?;

        // Decision 21 phase 2 quota：先看是否还有名额；超 → publish QuotaExceeded
        // 然后报错。get_or_create 是幂等（同 role 复用），所以已存在的 sandbox
        // 不算新建——只在物理目录还不存在时才 enforce。
        let target_sandbox_path = registry
            .root()
            .join(project_id.as_str())
            .join("sandboxes")
            .join(&role_for_sandbox);
        if !target_sandbox_path.exists() {
            let active = Self::count_active_workspaces(&registry, &project).await?;
            let limit = Self::project_sandbox_quota();
            if active >= limit {
                let _ = self.bus.publish(Event {
                    meta: EventMeta::now(),
                    kind: EventKind::WorkspaceQuotaExceeded {
                        project: project_id.clone(),
                        quota_kind: fuxi_core::QuotaKind::ConcurrentSandboxes,
                        requested: active + 1,
                        limit,
                    },
                });
                return Err(OrchestratorError::Other(format!(
                    "项目 {project_id} sandbox 配额已满（{active}/{limit}），\
                     先 retire 旧 sandbox 或调 FUXI_PROJECT_SANDBOX_QUOTA"
                )));
            }
            // Decision 21 phase 3 磁盘 quota——并发 sandbox 数过了再看磁盘上限。
            self.enforce_disk_quota(&registry, &project_id).await?;
        }

        // 2. 用 PersistentSandboxManager 起 / 复用 sandbox
        let mgr = fuxi_workspace::PersistentSandboxManager::new(project.clone(), registry.root());
        let sandbox_handle = mgr
            .get_or_create(&role_for_sandbox)
            .await
            .map_err(|e| OrchestratorError::Other(format!("sandbox 创建失败: {e}")))?;

        // 2.5 给门客 system_prompt 后段附「项目身份」段——让他知道自己住在
        // 哪个项目的 sandbox 里，调 `fuxi deliverable produce --project ...` 时
        // 自动用对的 slug。spawn 后所有 turn 都能看到这一段（agent 维度，跟
        // dispatch 时的 [FUXI_TASK_ID=...] task 维度互补）。
        // 注入到 profile.system_prompt 让 cc / codex 两 adapter 都生效——
        // codex compose_prompt 直接 prepend；cc launch_and_register 也读 profile。
        let project_context = format!(
            "\n\n## 项目身份（Decision 21）\n\n你住在项目 `{slug}` 的 L3 持久 sandbox：\n\
             - 工作目录：`{path}`\n\
             - 长期 branch：`{branch}`\n\
             - canonical 真项目：`{canonical}`\n\n\
             调 `fuxi deliverable produce` / `fuxi project list` 等命令时，\
             `--project` 参数填 `{slug}`。",
            slug = project.id,
            path = sandbox_handle.sandbox_path.display(),
            branch = sandbox_handle.branch,
            canonical = project.canonical_path.display(),
        );
        if profile.system_prompt.trim().is_empty() {
            profile.system_prompt = project_context.trim().to_string();
        } else {
            profile.system_prompt.push_str(&project_context);
        }

        // 3. 包成 borrowed WorkspaceHandle
        let agent_id = AgentId::new();
        self.publish_with_agent(
            agent_id,
            EventKind::AgentSpawning {
                role: profile.role.clone(),
                cli: kind.cli_tag().to_string(),
            },
        );
        // 顺带打 WorkspaceCreated 事件——本路径是 L3 sandbox 的实际产生点之一
        // （PersistentSandboxManager 不打事件，由 caller 包装；见 module doc 的
        // 设计取舍）。
        {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent_id);
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::WorkspaceCreated {
                    workspace_id: sandbox_handle.workspace_id.clone(),
                    project: project.id.clone(),
                    layer: fuxi_core::WorkspaceLayer::L3Persistent,
                    role: Some(role_for_sandbox.clone()),
                    task: None,
                    path: sandbox_handle.sandbox_path.clone(),
                },
            });
        }

        let handle = fuxi_core::workspace::WorkspaceHandle {
            agent: agent_id,
            repo_root: project.canonical_path.clone(),
            worktree_path: sandbox_handle.sandbox_path.clone(),
            branch: sandbox_handle.branch.clone(),
            borrowed: true, // L3 sandbox 长期保留，destroy 不动
        };
        info!(
            agent = %agent_id,
            role = %profile.role,
            project = %project.id,
            sandbox = %sandbox_handle.sandbox_path.display(),
            branch = %sandbox_handle.branch,
            "spawn worker in L3 persistent sandbox"
        );

        self.launch_and_register(agent_id, profile, kind, Some(handle))
            .await
    }

    /// Decision 21 phase 2：在已注册项目的 L2 ephemeral worktree 里 spawn 门客。
    ///
    /// 跟 `spawn_worker_in_project_sandbox` 关键差别：
    /// - 索引键：(project, task_id) 而非 (project, role)——一次性 task 一个 worktree
    /// - 路径：`<projects_root>/<project>/ephemeral/<task>/`
    /// - 分支：`task/<task-uuid>` 一次性
    /// - destroy 时**真**清（borrowed=false）；后续可被 archive→GC
    /// - 不复用：每次新 task 都新 worktree；多次 spawn 同 task → AlreadyExists
    ///
    /// **本方法仍 opt-in**：调用方明确知道走 L2 才传；玄女默认 dispatch 路径
    /// 仍走旧 spawn_worker / spawn_worker_in_project_sandbox。
    pub async fn spawn_worker_in_ephemeral_workspace(
        &self,
        profile: AgentProfile,
        kind: WorkerKind,
        project_id: fuxi_core::ProjectId,
        task: TaskId,
    ) -> Result<AgentId> {
        let registry_opt = self.project_registry.read().await.clone();
        let registry = registry_opt.ok_or_else(|| {
            OrchestratorError::Other(
                "project_registry 未注入 —— 调 Fuxi::set_project_registry 后再用".into(),
            )
        })?;
        let project = registry
            .get(&project_id)
            .await
            .map_err(|e| OrchestratorError::Other(format!("project lookup 失败: {e}")))?
            .ok_or_else(|| OrchestratorError::Other(format!("project {project_id} 未注册")))?;

        // Decision 21 phase 2 quota：跟 L3 spawn 同款 enforcement——L2 每次 create
        // 都是新 worktree（无幂等复用），故每次都查名额。
        let active = Self::count_active_workspaces(&registry, &project).await?;
        let limit = Self::project_sandbox_quota();
        if active >= limit {
            let _ = self.bus.publish(Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceQuotaExceeded {
                    project: project_id.clone(),
                    quota_kind: fuxi_core::QuotaKind::ConcurrentSandboxes,
                    requested: active + 1,
                    limit,
                },
            });
            return Err(OrchestratorError::Other(format!(
                "项目 {project_id} sandbox 配额已满（{active}/{limit}），\
                 先 retire 旧 sandbox 或调 FUXI_PROJECT_SANDBOX_QUOTA"
            )));
        }
        // Decision 21 phase 3 磁盘 quota
        self.enforce_disk_quota(&registry, &project_id).await?;

        let mgr = fuxi_workspace::EphemeralWorkspaceManager::new(project.clone(), registry.root());
        let ws_handle = mgr
            .create(task)
            .await
            .map_err(|e| OrchestratorError::Other(format!("ephemeral 创建失败: {e}")))?;

        let agent_id = AgentId::new();
        self.publish_with_agent(
            agent_id,
            EventKind::AgentSpawning {
                role: profile.role.clone(),
                cli: kind.cli_tag().to_string(),
            },
        );
        // WorkspaceCreated{layer=L2}——本路径是 L2 worktree 实际产生点。
        {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent_id);
            meta.task = Some(task);
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::WorkspaceCreated {
                    workspace_id: ws_handle.workspace_id.clone(),
                    project: project.id.clone(),
                    layer: fuxi_core::WorkspaceLayer::L2Ephemeral,
                    role: None,
                    task: Some(task),
                    path: ws_handle.workspace_path.clone(),
                },
            });
        }

        // L2 不像 L3 那样 borrowed=true（长期保留）；这里 borrowed=false 让 destroy
        // 真清。但 L2 的"完整生命"是 archive 而非 destroy——caller 在 task done 后
        // 调 EphemeralWorkspaceManager::archive（事件层 publish WorkspaceArchived）。
        // 当前 destroy 路径若被走（agent 异常死等），git worktree remove 会清掉，
        // archive 不会出现——接受这个 corner case，destroy 是异常清理通道。
        let handle = fuxi_core::workspace::WorkspaceHandle {
            agent: agent_id,
            repo_root: project.canonical_path.clone(),
            worktree_path: ws_handle.workspace_path.clone(),
            branch: ws_handle.branch.clone(),
            borrowed: false,
        };
        info!(
            agent = %agent_id,
            role = %profile.role,
            project = %project.id,
            %task,
            wt = %ws_handle.workspace_path.display(),
            "spawn worker in L2 ephemeral worktree"
        );

        self.launch_and_register(agent_id, profile, kind, Some(handle))
            .await
    }

    /// Decision 21 phase 2：把指定 task 的 L2 ephemeral workspace 归档。
    ///
    /// 跟 `spawn_worker_in_ephemeral_workspace` 配对——task 完成或进入归档窗口
    /// 时调本方法，物理 move ephemeral/<task> → archive/<task>，并 publish
    /// `WorkspaceArchived` 事件让 firehose / IM 看到。
    ///
    /// 调用时机由 caller 决定（v1 留给玄女或 dispatch hook 显式触发；v2 后续
    /// 加自动 hook on AgentDead）。
    pub async fn archive_l2_workspace(
        &self,
        project_id: fuxi_core::ProjectId,
        task: TaskId,
        reason: fuxi_core::ArchiveReason,
    ) -> Result<()> {
        let registry_opt = self.project_registry.read().await.clone();
        let registry = registry_opt.ok_or_else(|| {
            OrchestratorError::Other(
                "project_registry 未注入 —— 调 Fuxi::set_project_registry 后再用".into(),
            )
        })?;
        let project = registry
            .get(&project_id)
            .await
            .map_err(|e| OrchestratorError::Other(format!("project lookup 失败: {e}")))?
            .ok_or_else(|| OrchestratorError::Other(format!("project {project_id} 未注册")))?;
        let mgr = fuxi_workspace::EphemeralWorkspaceManager::new(project, registry.root());
        mgr.archive(task)
            .await
            .map_err(|e| OrchestratorError::Other(format!("archive 失败: {e}")))?;
        let workspace_id = fuxi_core::WorkspaceId::l2(&project_id, task);
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        let _ = self.bus.publish(Event {
            meta,
            kind: EventKind::WorkspaceArchived {
                workspace_id,
                reason,
            },
        });
        Ok(())
    }

    /// Bug 修：按 task 反查 L2 ephemeral 工作区并归档。bridge.rs 在 task 终态
    /// （Done/Cancelled）时触发——AgentDead 路径漏（门客 idle GC 走 / 状态机
    /// bug 卡 ShuttingDown 不死）的兜底。registry 未注入或没匹配项目都 silent
    /// Ok（大多数 task 不是 L2，不发 spurious 事件）。
    pub async fn archive_l2_for_task(
        &self,
        task: TaskId,
        reason: fuxi_core::ArchiveReason,
    ) -> Result<()> {
        let registry_opt = self.project_registry.read().await.clone();
        let Some(registry) = registry_opt else {
            return Ok(());
        };
        let projects = match registry.list().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "archive_l2_for_task: registry.list 失败");
                return Ok(());
            }
        };
        for project in projects {
            let mgr =
                fuxi_workspace::EphemeralWorkspaceManager::new(project.clone(), registry.root());
            if mgr.path_for(task).exists() {
                return self.archive_l2_workspace(project.id, task, reason).await;
            }
        }
        Ok(())
    }

    /// Decision 21 phase 2：把 L2 ephemeral 提升为 L3 持久 sandbox。
    ///
    /// 调用方一般是用户在 PWA 或 CLI 上明示「这次任务做得不错，留下 sandbox
    /// 继续用」。物理路径 / 分支重命名后 publish WorkspacePromoted。
    pub async fn promote_l2_to_l3(
        &self,
        project_id: fuxi_core::ProjectId,
        task: TaskId,
        to_role: String,
    ) -> Result<()> {
        let registry_opt = self.project_registry.read().await.clone();
        let registry = registry_opt.ok_or_else(|| {
            OrchestratorError::Other(
                "project_registry 未注入 —— 调 Fuxi::set_project_registry 后再用".into(),
            )
        })?;
        let project = registry
            .get(&project_id)
            .await
            .map_err(|e| OrchestratorError::Other(format!("project lookup 失败: {e}")))?
            .ok_or_else(|| OrchestratorError::Other(format!("project {project_id} 未注册")))?;
        let from_workspace_id = fuxi_core::WorkspaceId::l2(&project_id, task);
        let mgr = fuxi_workspace::EphemeralWorkspaceManager::new(project, registry.root());
        mgr.promote_to_l3(task, &to_role)
            .await
            .map_err(|e| OrchestratorError::Other(format!("promote 失败: {e}")))?;
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        let _ = self.bus.publish(Event {
            meta,
            kind: EventKind::WorkspacePromoted {
                from_workspace_id,
                to_role,
                project: project_id,
            },
        });
        Ok(())
    }

    /// 默认每项目最多并发 sandbox 数（L3 + active L2 之和）。
    /// 跟 Decision 21 phase 1 default 一致：8。
    /// 可通过 `FUXI_PROJECT_SANDBOX_QUOTA` env 覆盖。
    fn project_sandbox_quota() -> u64 {
        std::env::var("FUXI_PROJECT_SANDBOX_QUOTA")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(8)
    }

    /// 算项目当前 active workspace 数（L3 sandboxes + 未归档 L2 ephemerals）。
    /// quota 检查用——超过即拒新建并 publish WorkspaceQuotaExceeded。
    async fn count_active_workspaces(
        registry: &fuxi_workspace::FileSystemProjectRegistry,
        project: &fuxi_core::Project,
    ) -> Result<u64> {
        let l3_count =
            fuxi_workspace::PersistentSandboxManager::new(project.clone(), registry.root())
                .list()
                .await
                .map(|v| v.len() as u64)
                .unwrap_or(0);
        let l2_count =
            fuxi_workspace::EphemeralWorkspaceManager::new(project.clone(), registry.root())
                .list_active()
                .await
                .map(|v| v.len() as u64)
                .unwrap_or(0);
        Ok(l3_count + l2_count)
    }

    /// Decision 21 phase 3 磁盘 quota：每项目默认 5 GB。可由
    /// `FUXI_PROJECT_DISK_QUOTA_BYTES` env 覆盖（设 0 = 关闭检查）。
    fn project_disk_quota_bytes() -> u64 {
        std::env::var("FUXI_PROJECT_DISK_QUOTA_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(5 * 1024 * 1024 * 1024)
    }

    /// 缓存 TTL——同 project 60s 内重复 spawn 不再扫盘。可调
    /// `FUXI_PROJECT_DISK_CACHE_SECS` 覆盖。
    fn project_disk_cache_secs() -> u64 {
        std::env::var("FUXI_PROJECT_DISK_CACHE_SECS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(60)
    }

    /// 拿到（cache hit 或重算）项目当前总占盘字节数。
    async fn measure_project_disk_bytes(
        &self,
        registry: &fuxi_workspace::FileSystemProjectRegistry,
        project_id: &fuxi_core::ProjectId,
    ) -> u64 {
        let ttl = std::time::Duration::from_secs(Self::project_disk_cache_secs());
        {
            let cache = self.disk_quota_cache.read().await;
            if let Some((measured_at, bytes)) = cache.get(project_id)
                && measured_at.elapsed() < ttl
            {
                return *bytes;
            }
        }
        let project_root = registry.root().join(project_id.as_str());
        // 阻塞 fs walk 放进 spawn_blocking——递归扫多 GB sandbox 不能在 async runtime 里直跑。
        let bytes = match tokio::task::spawn_blocking(move || dir_size_bytes(&project_root)).await {
            Ok(n) => n,
            Err(e) => {
                warn!(project = %project_id, error = %e, "disk size 任务 panic，回 0 跳过 quota");
                0
            }
        };
        let mut cache = self.disk_quota_cache.write().await;
        cache.insert(project_id.clone(), (std::time::Instant::now(), bytes));
        bytes
    }

    /// spawn 路径共享的"先看磁盘 quota，超 → publish + Err"。
    async fn enforce_disk_quota(
        &self,
        registry: &fuxi_workspace::FileSystemProjectRegistry,
        project_id: &fuxi_core::ProjectId,
    ) -> Result<()> {
        let limit = Self::project_disk_quota_bytes();
        if limit == 0 {
            return Ok(());
        }
        let used = self.measure_project_disk_bytes(registry, project_id).await;
        if used >= limit {
            let _ = self.bus.publish(Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceQuotaExceeded {
                    project: project_id.clone(),
                    quota_kind: fuxi_core::QuotaKind::DiskBytes,
                    requested: used,
                    limit,
                },
            });
            return Err(OrchestratorError::Other(format!(
                "项目 {project_id} 磁盘占用已超 quota（{used}B / {limit}B），\
                 先 retire 旧 sandbox + GC archive 或调 FUXI_PROJECT_DISK_QUOTA_BYTES"
            )));
        }
        Ok(())
    }

    /// **测试 / GC 触发后**主动失效缓存——下次 spawn 重扫。
    pub async fn invalidate_disk_quota_cache(&self, project_id: &fuxi_core::ProjectId) {
        self.disk_quota_cache.write().await.remove(project_id);
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
        // #48 memory-v2 注入——只在 set_memory_stores 注入过 + role 不在黑名单
        // （xuannv/extractor/cangjie）时拉用户身份卡 + 同 role 心法拼 system prompt。
        // 没设 stores 时整段跳过——保持本 method 在测试 / 早期启动场景可单测无依赖。
        let memory_stores = self.memory_stores.read().await.clone();

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
                // #48 memory-v2 注入——sentinel addendum 之后追加身份卡 + 心法。
                // 失败时 warn 不挂——memory 注入失败应降级为"裸 spawn"而非整 spawn 失败。
                if let Some(stores) = memory_stores.as_ref()
                    && let Err(e) = crate::sentinel_addendum::inject_role_memory_cc(
                        &mut cc_cfg,
                        &profile.role,
                        &stores.user_profile,
                        &stores.hetu,
                    )
                    .await
                {
                    warn!(error = %e, role = %profile.role, "memory-v2 cc 注入失败，降级裸 spawn");
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
                // #48 memory-v2 注入——同 cc 思路，落 profile.system_prompt 末尾。
                // 把 role 先 clone 出来，再 `&mut profile` 借用——否则 borrow 冲突。
                if let Some(stores) = memory_stores.as_ref() {
                    let role_for_memory = profile.role.clone();
                    if let Err(e) = crate::sentinel_addendum::inject_role_memory_codex(
                        &mut profile,
                        &role_for_memory,
                        &stores.user_profile,
                        &stores.hetu,
                    )
                    .await
                    {
                        warn!(error = %e, role = %role_for_memory, "memory-v2 codex 注入失败，降级裸 spawn");
                    }
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
    /// 决策 13 + 22 配套：派活时给门客 prompt 前置一行
    /// `[FUXI_TASK_ID=task-<uuid>]`，让门客调 `fuxi deliverable produce --task ...`
    /// 时知道当前 task uuid（不必玄女主动告诉）。
    ///
    /// 黑名单：xuannv / extractor 不注入——
    /// - xuannv 是接收方（user-turn 类 task description 是用户原文，注入会污染对话）
    /// - extractor 是幕后工，跟 deliverable 模型无关
    ///
    /// 跟 sentinel addendum 黑名单同步。
    fn maybe_inject_task_id(role: &str, mut task: Task) -> Task {
        if !crate::sentinel_addendum::should_inject_for_role(role) {
            return task;
        }
        // 单独一行 + 分隔空行——LLM grep 友好，不污染 markdown 渲染
        let prefix = format!("[FUXI_TASK_ID={}]\n\n", task.id);
        task.description = format!("{prefix}{}", task.description);
        task
    }

    pub async fn dispatch(&self, agent_id: AgentId, task: Task) -> Result<()> {
        // Decision 22 phase 1：把 [FUXI_TASK_ID=...] 注入要送给 agent 的 task copy。
        // 事件流（TaskCreated 等）仍用**原** description——审计保真，不被平台
        // marker 污染。注入只为让门客 Bash 里跑 `fuxi deliverable produce --task`
        // 能 grep 到 task uuid。黑名单（xuannv / extractor）跳过。
        let inject_role = self
            .shelf
            .get_agent(agent_id)
            .await
            .map(|a| a.card().profile.role.clone());

        // v2 跨节点 sandbox：task 关联到 project 但用户没显式 pin → 按
        // project.host_nodes 选最闲节点 auto-pin。这一步必须在 needs_dist
        // 决策**之前**——auto_pin 改写后，task.pinned_node 也将是 Some(...)，
        // 自然进 dist 路径。registry / provider 未注入 / host_nodes 空 / 候选全离线
        // 时 auto_pin 返 None，dispatch 走原来的本地 spawn 路径（向后兼容）。
        let mut task = task;
        if let Some(picked) = self.auto_pin_from_project(&task).await {
            info!(
                task_id = %task.id,
                project_id = ?task.project_id,
                picked_node = %picked,
                "dispatch v2: auto-pin from project.host_nodes（最闲节点）"
            );
            task.pinned_node = Some(picked);
        }

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
        // issue f4e0ff39：`--to <id>` 指向本地已存在的 agent 时，caller（用户/玄女）
        // 是**显式点名**这个 agent 干活——直派本地，不进 dist queue。否则 home 既是
        // controller 又是唯一 worker 且**无 pull loop**（im_dist 虚节点只注册不消费）
        // 时，required_tags 让 task 进 dist queue 无人 pull，9 分钟卡死后 idle_ttl
        // 把 agent GC 掉、task 永久悬空。pinned_node 仍**永远**走 dist——那是玄女
        // 显式跨节点路由的唯一信号，不能被本地 agent 的存在抹掉。
        let agent_is_local = self.shelf.get_agent(agent_id).await.is_some();
        let needs_dist =
            task.pinned_node.is_some() || (!task.required_tags.is_empty() && !agent_is_local);
        if !needs_dist && !task.required_tags.is_empty() {
            info!(
                task_id = %task.id,
                agent = %agent_id,
                required_tags = ?task.required_tags,
                "dispatch routing: --to 指向本地 agent，override required_tags 直派本地"
            );
        }
        if needs_dist {
            let enqueuer_opt = self.dist_enqueuer.read().await.clone();
            if let Some(enqueuer) = enqueuer_opt {
                // dist 路径合成 worker system prompt——含 role 心智 + sentinel
                // addendum（决策 13）。本地 spawn 路径走 inject_cc 在 spawn_worker
                // 处注 cc launch config；dist 路径远端 cc 由 worker `run_cc_job`
                // 经 `--append-system-prompt` 注，必须从这里把整段串好下发。
                // 否则远端跑裸 cc，玄女永远等不到 deliverable nudge（#4 真因）。
                let agent_for_role = self.shelf.get_agent(agent_id).await;
                let role_for_worker = agent_for_role
                    .as_ref()
                    .map(|a| a.card().profile.role.clone());
                let system_prompt = agent_for_role.as_ref().and_then(|a| {
                    let profile = &a.card().profile;
                    let want_sentinel = !crate::sentinel_addendum::is_globally_disabled()
                        && crate::sentinel_addendum::should_inject_for_role(&profile.role);
                    if !want_sentinel && profile.system_prompt.trim().is_empty() {
                        return None;
                    }
                    let role_part = profile.system_prompt.trim();
                    let sentinel_part = crate::sentinel_addendum::SENTINEL_ADDENDUM_TEXT.trim();
                    let assembled = match (role_part.is_empty(), want_sentinel) {
                        (true, true) => sentinel_part.to_string(),
                        (false, true) => format!("{role_part}\n\n{sentinel_part}"),
                        (false, false) => role_part.to_string(),
                        (true, false) => return None,
                    };
                    Some(assembled)
                });
                info!(
                    task_id = %task.id,
                    pinned_node = ?task.pinned_node,
                    required_tags = ?task.required_tags,
                    has_system_prompt = system_prompt.is_some(),
                    role = ?role_for_worker,
                    "dispatch routing: 走 dist enqueue（spec gap e）"
                );
                // dist 路径补发 TaskCreated（#1 真因：远端 cc 不发 task lifecycle，
                // 没人填 acc.title / member_ids，aggregate 把整 task 当空壳过滤）。
                // 跟本地路径对称发 TaskCreated；TaskDispatched 留 None target——
                // 让 ingest 用 meta.agent+meta.task fallback 累积 member_ids。
                {
                    let mut meta = EventMeta::now();
                    meta.task = Some(task.id);
                    let _ = self.bus.publish(Event {
                        meta,
                        kind: EventKind::TaskCreated {
                            title: task.title.clone(),
                            description: task.description.clone(),
                        },
                    });
                }
                // #76：透传 home 真相 task_id 让 worker 端 cc events meta.task 一致；
                // #77：透传 role 让 worker 端发 AgentSpawning 给 home aggregate 拿 role
                let opts = crate::DistEnqueueOptions {
                    system_prompt,
                    task_id: Some(task.id.to_string()),
                    role: role_for_worker,
                };
                // dist 路径也注入——远端 worker 跑 cc，鲁班 prompt 应能看到 task_id
                let task_for_worker = match &inject_role {
                    Some(r) => Self::maybe_inject_task_id(r, task),
                    None => task,
                };
                let _job_id = enqueuer.enqueue(&task_for_worker, opts).await?;
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
        // FU-2 收尾：task 下面会被 move 进 agent.dispatch，先把 topic 捞出来给 pump
        // 闭包（always-nudge 兜底 AgentRequestReview 用它 stamp meta.topic_id）。
        let task_topic_id = task.topic_id;
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

        // 本地 spawn 路径：注入 [FUXI_TASK_ID=...] 到送给 agent 的 task copy
        // （事件已发了原 description，agent 看到带 marker 的版本不污染审计）。
        let task_for_agent = match inject_role {
            Some(r) => Self::maybe_inject_task_id(&r, task),
            None => task,
        };
        let mut rx = agent.dispatch(task_for_agent).await?;
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
            // #79 / 本地路径同款 always-nudge 兜底：cc haiku 实测不可靠遵守
            // sentinel addendum——本地 spawn 的鲁班跑完 task 不主动发 sentinel
            // 时玄女永远等不到通知（用户 19:49 测「测试交付」实证）。pump 退出
            // 时若没观察到 AgentRequestReview，且 task ok done，兜底发一条让
            // 玄女能开口告交付。Decision 13 "门客自决" 留给 model 升级到能可
            // 靠遵守 addendum 时再恢复（FUXI_DISABLE_NUDGE_FALLBACK=1 可关）。
            let mut saw_review_request = false;
            let mut last_assistant_text: Option<String> = None;
            let mut task_done_ok = false;
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
                // issue 8ebff743：terminal 后的 wind-down 事件——`UsageReport`
                // （cc result 几乎必带 usage）/ `ThinkingFinished`（ws_pump 在 turn
                // terminal 处兜底追加）——是**同一 turn 的收尾**，绝不代表新 turn
                // 启动。它们不得重置 `saw_terminal`，否则 pump 退回无超时
                // `rx.recv()`，cc 持久 WS 仍活、rx 不关时永久卡死、shelf 锁死 Busy。
                let is_winddown = matches!(
                    &ev.kind,
                    EventKind::UsageReport { .. } | EventKind::ThinkingFinished
                );
                // #79 兜底：track sentinel + final text + done state
                if matches!(&ev.kind, EventKind::AgentRequestReview { .. }) {
                    saw_review_request = true;
                }
                if let EventKind::AgentResponded { text, .. } = &ev.kind {
                    last_assistant_text = Some(text.clone());
                }
                if matches!(
                    &ev.kind,
                    EventKind::TaskStateChanged {
                        to: fuxi_core::task::TaskState::Done,
                        ..
                    }
                ) {
                    task_done_ok = true;
                }
                if bus.publish(ev).is_err() {
                    warn!(agent = %agent_id, "event bus 已关闭，pump 退出");
                    break;
                }
                if is_terminal {
                    saw_terminal = true;
                } else if saw_terminal && !is_winddown {
                    // terminal 后收到**非 wind-down** 新事件 = M2.1 pending-drain
                    // 的新 turn 启动了（cc 收 follow-up 后重新 thinking/响应）；
                    // 重置回无超时等待，追到新 turn 的 terminal。wind-down 事件
                    // 走 is_winddown 豁免不触发重置（issue 8ebff743）。
                    saw_terminal = false;
                }
            }
            // #79 always-nudge fallback：task done 但 cc 没主动发 sentinel
            // → pump 兜底 publish AgentRequestReview，玄女能开口告交付。
            // 走 bus 而非 trait method 因为后者要 active_tx，已 Idle 的 cc 接不了。
            //
            // **跳过 xuannv role**：玄女不是门客，user-turn task 完成时不该
            // 给自己发 nudge——否则 bridge 转 AgentRequestReview→intervene 玄女
            // →新 turn 又触发 always-nudge→死循环（用户实测 21:31「这个 xuannv
            // 门客在循环把我的话回放给我」根因）。同 sentinel_addendum 黑名单
            // 语义对齐——玄女是 sentinel 接收方，不是发送方。
            let role_is_xuannv = recall_agent.card().profile.role == "xuannv";
            let nudge_disabled =
                std::env::var("FUXI_DISABLE_NUDGE_FALLBACK").ok().as_deref() == Some("1");
            if task_done_ok && !saw_review_request && !nudge_disabled && !role_is_xuannv {
                let summary = match &last_assistant_text {
                    Some(t) if !t.trim().is_empty() => {
                        let trimmed = t.trim();
                        let count = trimmed.chars().count();
                        if count <= 200 {
                            trimmed.to_string()
                        } else {
                            let truncated: String = trimmed.chars().take(199).collect();
                            format!("{truncated}…")
                        }
                    }
                    _ => "任务已完成".to_string(),
                };
                let mut meta = EventMeta::now();
                meta.agent = Some(agent_id);
                meta.task = Some(task_id);
                // FU-2 收尾：兜底信号归位发起 task 的 topic，跟适配器 emit 的 worker
                // 事件口径一致（meta.topic_id），订阅方按 topic 过滤不漏完工信号。
                meta.topic_id = task_topic_id;
                let _ = bus.publish(Event {
                    meta,
                    kind: EventKind::AgentRequestReview {
                        agent: agent_id,
                        task: task_id,
                        deliverable_kind: fuxi_core::event::DeliverableKind::ResearchSummary,
                        summary,
                        artifact_ref: None,
                    },
                });
                info!(
                    %agent_id,
                    %task_id,
                    "always-nudge fallback：cc 未输出 sentinel，pump 兜底发 AgentRequestReview"
                );
            }
            // Bug 修：pump 无终态退出 → 兜底 emit TaskCancelled 防 task 永远显
            // "running"。常见触发：cc 进程崩溃 / GC 走门客时 rx 端关闭 / agent
            // adapter 出错。事件库实证 task-fb7437a8 cangjie-extract 撞过——
            // agent 29dafabc 后期无任何事件，PWA 永远卡 running。
            //
            // 不发送 AgentDead——pump 不掌握"死因"语义，AgentDead 由 shutdown
            // 路径或 worker 心跳超时机制自管。这里只补 TaskStateChanged 让任务
            // 视图收敛。下游 archive_l2_for_task / always-nudge 已对 Cancelled
            // 做过路径处理（archive 幂等，nudge 已 gate 在 task_done_ok）。
            if !saw_terminal {
                let mut meta = EventMeta::now();
                meta.agent = Some(agent_id);
                meta.task = Some(task_id);
                let _ = bus.publish(Event {
                    meta,
                    kind: EventKind::TaskStateChanged {
                        from: fuxi_core::task::TaskState::InProgress,
                        to: fuxi_core::task::TaskState::Cancelled,
                    },
                });
                warn!(
                    %agent_id,
                    %task_id,
                    "dispatch pump 无终态退出 → 兜底 emit TaskCancelled (orphan recovery)"
                );
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
    #[allow(clippy::too_many_arguments)]
    pub async fn intervene(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
        mentions: Vec<AgentId>,
        pinned_node: Option<String>,
        attachments: Vec<String>,
        task_id: Option<TaskId>,
    ) -> Result<()> {
        // 用户主动 intervene 路径——不带 system_origin（None）。
        // bug #76：bridge / sentinel 系统注入走 [`intervene_system_origin`]。
        // Bug B · `task_id` 来自 PWA 任务 thread 上下文；写入所有 publish 事件的
        // meta.task，让 `task_thread_visible` filter 能拉回这条用户消息。系统注入
        // (`intervene_system_origin`) 维持 task_id=None。
        self.intervene_inner(
            agent_id,
            interrupt_first,
            text,
            mentions,
            pinned_node,
            attachments,
            None,
            task_id,
        )
        .await
    }

    /// bug #76 · 系统注入入口（bridge / sentinel addendum 用）。
    ///
    /// 与 [`Self::intervene`] 唯一差别：在 `UserInterventionSent` 上挂
    /// `system_origin: Some(<标记>)`，告诉前端这条不是用户敲的，应渲染成
    /// 玄女侧的「系统消息」气泡（不是右侧 user bubble）。
    ///
    /// 标记取值（snake_case，跟前端 reducer 对齐）：
    /// `"agent_dead"` / `"trigger_fired"` / `"review_request"` / `"review_timeout"` 等。
    pub async fn intervene_system_origin(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
        system_origin: String,
    ) -> Result<()> {
        self.intervene_inner(
            agent_id,
            interrupt_first,
            text,
            Vec::new(),
            None,
            Vec::new(),
            Some(system_origin),
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn intervene_inner(
        &self,
        agent_id: AgentId,
        interrupt_first: bool,
        text: &str,
        mentions: Vec<AgentId>,
        pinned_node: Option<String>,
        attachments: Vec<String>,
        system_origin: Option<String>,
        task_id: Option<TaskId>,
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
                meta.task = task_id;
                let id = meta.id;
                let _ = self.bus.publish(Event {
                    meta,
                    kind: EventKind::UserInterventionSent {
                        target: agent_id,
                        mode: "append_via_dispatch".to_string(),
                        text: text.to_string(),
                        mentions: mentions.clone(),
                        pinned_node: pinned_node.clone(),
                        attachments: attachments.clone(),
                        system_origin: system_origin.clone(),
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
            // 块3.1：若 intervene 目标本身是某个**玄女分身**（idle-degrade user-turn，
            // 如 TUI Submit::Xuannv / PWA 对分身发话），把她服务的 topic 打到 task 上，
            // 让这轮对话产生的事件 meta.topic_id 归位该 topic（门客适配器 emit 时已读
            // task.topic_id → meta.topic_id）。`topic_of` None = 目标是普通门客（用户
            // 直接 intervene 闲置门客）——发起方 topic 此入口拿不到，留块5 给玄女分身
            // 子进程注入 topic env 后由 `fuxi dispatch` 带上，本处不硬猜。
            if let Some(topic) = self.xuannv_pool.topic_of(agent_id).await {
                task = task.with_topic_id(topic);
            }
            self.dispatch(agent_id, task).await?;
            // 抄送玄女
            let xuannv = self.xuannv_id().await;
            if let Some(xn) = xuannv
                && xn != agent_id
            {
                let mut meta = EventMeta::now();
                meta.agent = Some(xn);
                meta.task = task_id;
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
            meta.task = task_id;
            let id = meta.id;
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::UserInterventionSent {
                    target: agent_id,
                    mode: mode_str.to_string(),
                    text: text.to_string(),
                    mentions,
                    pinned_node,
                    attachments,
                    system_origin,
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
            self.publish_with_agent_task(
                agent_id,
                task_id,
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
        self.publish_with_agent_task(
            agent_id,
            task_id,
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
            meta.task = task_id;
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

    /// Task-scoped 门客消息。
    ///
    /// 这是 mailbox 的编排入口：先写 `AgentMessageQueued`，再尝试投递到目标 agent，
    /// 成功写 `AgentMessageDelivered`，失败写 `AgentMessageFailed` 并把原错误返回给调用方。
    /// 所有事件都挂同一个 task，后续 UI/审计/远端同步只读 EventBus。
    pub async fn send_agent_message(
        &self,
        task_id: TaskId,
        from: AgentId,
        to: AgentId,
        text: &str,
        summary: Option<String>,
    ) -> Result<uuid::Uuid> {
        let message_id = crate::mailbox::queue_agent_message(
            &self.bus,
            task_id,
            from,
            to,
            text.to_string(),
            summary,
        )?;
        let Some(agent) = self.shelf.get_agent(to).await else {
            let err = OrchestratorError::AgentNotFound(to);
            let _ = crate::mailbox::mark_agent_message_failed(
                &self.bus,
                task_id,
                message_id,
                from,
                to,
                err.to_string(),
            );
            return Err(err);
        };
        if let Err(e) = agent.send_message(task_id, text).await {
            let _ = crate::mailbox::mark_agent_message_failed(
                &self.bus,
                task_id,
                message_id,
                from,
                to,
                e.to_string(),
            );
            return Err(e.into());
        }
        crate::mailbox::mark_agent_message_delivered(&self.bus, task_id, message_id, from, to)?;
        Ok(message_id)
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
        // 块2：豁免从"单 general 玄女"升级为"池中任一活分身"——每个 topic 的分身
        // 都是该话题的用户对话入口，误 kill 等价旧的单玄女被杀 bug。dormant 回收
        // （idle_gc）会**先** pool.remove 再走 shutdown_idle，那时 is_active_clone
        // 已为 false，正常回收进程；本豁免只拦"映射还在却要永久 kill"的误路径。
        if self.xuannv_pool.is_active_clone(id).await {
            warn!(
                agent = %id,
                reason = %reason,
                "shutdown_agent: 拒绝杀玄女分身（豁免）——dormant 回收须先 pool.remove"
            );
            return Ok(());
        }
        self.shutdown_agent_inner(id, reason).await
    }

    /// task #8 上下文交接专用——绕过 [`Self::shutdown_agent`] 的玄女豁免，强 kill
    /// 当前玄女副本，调用方紧接着 `set_xuannv(new_id)` 把新副本接上。
    ///
    /// **不要在别处调用**：玄女豁免是核心防御（GC / 误 kill 不该误伤她）。本路径
    /// 是用户**显式**主动交接（写了 handoff），等价于「我自己要换副本」。
    pub async fn shutdown_xuannv_for_handoff(&self, id: AgentId, reason: String) -> Result<()> {
        info!(agent = %id, reason = %reason, "shutdown_xuannv_for_handoff: 用户主动交接，绕过豁免");
        // 块2：先把这个分身从池里摘掉，否则 is_active_clone 会对已死 id 持续返 true，
        // 也避免 watch 订阅者拿到将死的 id。caller 紧接着 set_xuannv(new) 重建映射。
        // general 绑定即便不摘，后续 set_xuannv 也会覆盖；非 general 交接则必须摘。
        if let Some(topic) = self.xuannv_pool.topic_of(id).await {
            self.xuannv_pool.remove(topic).await;
        }
        self.shutdown_agent_inner(id, reason).await
    }

    async fn shutdown_agent_inner(&self, id: AgentId, reason: String) -> Result<()> {
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
        self.publish_with_agent_task(agent, None, kind);
    }

    /// 同 `publish_with_agent` 但额外挂 `meta.task`。Bug B 修：intervene 路径要
    /// 把 PWA 任务 thread 的 `task_id` 写到 `AgentInterrupted` /
    /// `TaskInterventionApplied` 等事件，让 `task_thread_visible` filter 拉得回。
    fn publish_with_agent_task(&self, agent: AgentId, task_id: Option<TaskId>, kind: EventKind) {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        meta.task = task_id;
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

/// 块5：general 镜像 reconciler——把 `xuannv_id` watch 始终同步成池里 general 分身。
///
/// WHY：`xuannv_id` watch 是兼容壳 [`Fuxi::xuannv_id`] + bridge general fallback +
/// conv_store sync 的真相来源，但池的 **remove**（dormant 回收 / handoff）路径不写
/// 它——只 `set_xuannv_for_topic` 写。后果（今天的黑洞坑）：general 分身被 idle_gc
/// dormant 回收后，镜像仍指已死 id，用户消息打到死分身。让池做唯一真相源：订
/// `pool.watch()`，每次快照变化就把镜像设成 `snapshot.get(general)`（含回收后 None）。
///
/// 与 `set_xuannv_for_topic` 的同步直写并存：set 路径两边写同一值幂等；remove 路径
/// 只有本 reconciler 写（置 None）。`send_if_modified` 避免无谓 changed 通知。
fn spawn_general_mirror_sync(
    mut pool_rx: watch::Receiver<HashMap<fuxi_core::TopicId, AgentId>>,
    xuannv_tx: watch::Sender<Option<AgentId>>,
) {
    let general = fuxi_core::TopicId::general();
    tokio::spawn(async move {
        loop {
            // 先按当前快照对齐一次（启动期 / 漏掉的变更兜底），再等下次变化。
            let desired = pool_rx.borrow().get(&general).copied();
            xuannv_tx.send_if_modified(|cur| {
                if *cur != desired {
                    *cur = desired;
                    true
                } else {
                    false
                }
            });
            if pool_rx.changed().await.is_err() {
                debug!("general_mirror_sync: 池 watch 关闭，退出");
                break;
            }
        }
    });
}

/// Decision 21 phase 3：递归求 `path` 子树所有 regular file 的字节数和。
///
/// - 不跟 symlink（避免 `<projects_root>/<project>/sandboxes/<role>` worktree 里的
///   symlink 跳出 sandbox 双计 / 死循环）
/// - 单条 entry 元数据失败 silent skip 不传错——quota 估算允许少算
pub(crate) fn dir_size_bytes(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            // symlink_metadata 走 std::fs 模块函数路径——DirEntry::metadata 会
            // 跟 symlink，dir entries 没有 symlink_metadata 方法。
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let ft = meta.file_type();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
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

#[cfg(test)]
mod disk_size_tests {
    use super::*;

    #[test]
    fn dir_size_bytes_recurses_and_skips_symlinks() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("a.bin"), vec![0u8; 1024]).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("b.bin"), vec![0u8; 2048]).unwrap();

        // symlink 指向同一根 → 必须不死循环 / 不重复计数
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(dir.path(), dir.path().join("loop"));
        }

        let total = dir_size_bytes(dir.path());
        assert_eq!(total, 1024 + 2048);
    }

    #[test]
    fn dir_size_bytes_returns_zero_for_missing_path() {
        let dir = tempfile::tempdir().expect("tmp");
        let missing = dir.path().join("does-not-exist");
        assert_eq!(dir_size_bytes(&missing), 0);
    }
}

#[cfg(test)]
mod project_sandbox_tests {
    //! `spawn_worker_in_project_sandbox` 失败路径单测——不实际 launch cc/codex
    //! （需要二进制 + cwd），只验输入校验。完整 e2e 留 integration test。

    use super::*;
    use fuxi_agent_codex::CodexLaunchConfig;
    use fuxi_core::ProjectId;
    use fuxi_core::agent::AgentProfile;
    use fuxi_events::EventBus;
    use fuxi_workspace::{FileSystemProjectRegistry, GitWorktreeWorkspace};

    async fn make_fuxi() -> (tempfile::TempDir, Arc<Fuxi>) {
        let dir = tempfile::tempdir().unwrap();
        // 假 workspace—— spawn_worker_in_project_sandbox 不会用它（用
        // PersistentSandboxManager），这里只为构造 Fuxi 占位
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let bus = EventBus::with_memory_store().await.unwrap();
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        (dir, fuxi)
    }

    fn dummy_profile(role: &str) -> AgentProfile {
        AgentProfile {
            name: format!("test-{role}"),
            role: role.to_string(),
            cli: "codex".to_string(),
            system_prompt: String::new(),
            tags: Vec::new(),
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn errors_when_registry_not_injected() {
        let (_dir, fuxi) = make_fuxi().await;
        let err = fuxi
            .spawn_worker_in_project_sandbox(
                dummy_profile("luban"),
                WorkerKind::Codex(CodexLaunchConfig::default()),
                ProjectId::new("erp").unwrap(),
                "luban".into(),
            )
            .await
            .expect_err("未注入 registry 应失败");
        assert!(
            err.to_string().contains("project_registry 未注入"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn errors_when_project_not_registered() {
        let (_dir, fuxi) = make_fuxi().await;
        let registry_root = tempfile::tempdir().unwrap();
        fuxi.set_project_registry(Arc::new(FileSystemProjectRegistry::new(
            registry_root.path(),
        )))
        .await;

        let err = fuxi
            .spawn_worker_in_project_sandbox(
                dummy_profile("luban"),
                WorkerKind::Codex(CodexLaunchConfig::default()),
                ProjectId::new("ghost").unwrap(),
                "luban".into(),
            )
            .await
            .expect_err("未注册 project 应失败");
        assert!(err.to_string().contains("未注册"), "got: {err}");
    }
}

#[cfg(test)]
mod task_id_injection_tests {
    use super::*;
    use fuxi_core::TaskId;

    /// luban / 普通门客 dispatch 时 description 头部应被注入 [FUXI_TASK_ID=...]，
    /// 让它在 Bash 里跑 `fuxi deliverable produce --task` 能从 prompt grep 拿。
    #[test]
    fn injects_task_id_for_luban_role() {
        let original = "修 ERP-1066";
        let task_id = TaskId::new();
        let task = {
            let mut t = Task::new("title", original);
            t.id = task_id;
            t
        };
        let injected = Fuxi::maybe_inject_task_id("luban", task);
        assert!(
            injected
                .description
                .starts_with(&format!("[FUXI_TASK_ID=task-{}]", task_id.0)),
            "description 应被注入 task_id 头：{}",
            injected.description
        );
        assert!(
            injected.description.contains(original),
            "原 description 应保留：{}",
            injected.description
        );
    }

    /// xuannv / extractor 黑名单——dispatch 给玄女的 user-turn task 不该被污染
    /// （description 是用户原文，前置 [FUXI_TASK_ID=...] 会让玄女对话上下文混入
    /// 平台 marker，影响她的 LLM 推理）。
    #[test]
    fn skips_blacklisted_roles() {
        let task = Task::new("user-turn", "你好玄女");
        let original_desc = task.description.clone();
        let after = Fuxi::maybe_inject_task_id("xuannv", task);
        assert_eq!(
            after.description, original_desc,
            "xuannv 不应被注入 task_id"
        );

        let task2 = Task::new("extract", "提取这个对话");
        let original_desc2 = task2.description.clone();
        let after2 = Fuxi::maybe_inject_task_id("extractor", task2);
        assert_eq!(
            after2.description, original_desc2,
            "extractor 不应被注入 task_id"
        );
    }

    /// 注入是有边界的：只前置一次，不重复（同一 task 二次 dispatch 不该叠两层
    /// FUXI_TASK_ID）。当前实装是无状态拼接——若已存在则会重复，留 todo。
    /// 本测试目前只验"一次注入正确"，重复注入的防护留 phase 2。
    #[test]
    fn injection_format_single_line_with_blank_separator() {
        let task = Task::new("t", "body");
        let after = Fuxi::maybe_inject_task_id("luban", task);
        // 单独一行（结尾 `\n\n`）——LLM grep 友好
        assert!(after.description.contains("]\n\nbody"));
    }
}

#[cfg(test)]
mod pump_orphan_recovery_tests {
    //! Bug 修测试：dispatch pump 无终态退出时兜底 emit TaskCancelled。

    use super::*;
    use async_trait::async_trait;
    use futures_util::StreamExt;
    use fuxi_core::Result as CoreResult;
    use fuxi_core::agent::{AgentCard, AgentProfile, AgentStatus};
    use fuxi_events::EventBus;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio::sync::mpsc;

    /// 受控 agent：dispatch() 返回测试持有的 rx；test 端 drop sender 即触发 pump 退出。
    /// agent 本身不持 sender——避免 rx 永远不关闭。
    struct ControllableAgent {
        card: AgentCard,
        rx_holder: Mutex<Option<mpsc::Receiver<Event>>>,
    }
    impl ControllableAgent {
        fn new(role: &str) -> (Arc<Self>, mpsc::Sender<Event>) {
            let (tx, rx) = mpsc::channel(64);
            let agent = Arc::new(Self {
                card: AgentCard {
                    id: AgentId::new(),
                    profile: AgentProfile {
                        name: format!("ctrl-{role}"),
                        role: role.to_string(),
                        cli: "stub".to_string(),
                        system_prompt: String::new(),
                        tags: vec![],
                        extra: Default::default(),
                    },
                    endpoint: "stub://".into(),
                    status: AgentStatus::Idle,
                },
                rx_holder: Mutex::new(Some(rx)),
            });
            (agent, tx)
        }
    }
    #[async_trait]
    impl Agent for ControllableAgent {
        fn card(&self) -> &AgentCard {
            &self.card
        }
        async fn dispatch(&self, _task: Task) -> CoreResult<mpsc::Receiver<Event>> {
            let rx = self
                .rx_holder
                .lock()
                .await
                .take()
                .expect("dispatch: rx 已被取走");
            Ok(rx)
        }
        async fn send_message(&self, _t: TaskId, _text: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn cancel(&self, _t: TaskId) -> CoreResult<()> {
            Ok(())
        }
        async fn shutdown(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    async fn make_fuxi() -> Arc<Fuxi> {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let bus = EventBus::with_memory_store().await.unwrap();
        // dir 作 workspace 占位；测试不申请 worktree
        std::mem::forget(dir);
        Arc::new(Fuxi::new(bus, ws))
    }

    /// pump 无终态退出（rx 直接被关闭无终态事件） → 兜底 emit TaskCancelled。
    #[tokio::test]
    async fn pump_orphan_emits_task_cancelled_on_no_terminal() {
        let fuxi = make_fuxi().await;
        let bus = fuxi.bus();
        let mut sub = bus.subscribe();

        let (agent, tx) = ControllableAgent::new("luban");
        let _agent_id = fuxi.insert_agent(agent.clone(), None).await;

        let task = Task::new("test-task", "noop");
        let task_id = task.id;
        fuxi.dispatch(agent.card().id, task)
            .await
            .expect("dispatch");

        // 给 pump 一点时间 spawn 起来 + 完成 dispatch 内的事件
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // 关 sender → rx None → pump break 退出（无终态事件）
        drop(tx);

        // 等 TaskCancelled 兜底事件
        let mut found = false;
        for _ in 0..50 {
            match tokio::time::timeout(std::time::Duration::from_millis(100), sub.next()).await {
                Ok(Some(Ok(ev))) => {
                    if matches!(
                        &ev.kind,
                        EventKind::TaskStateChanged {
                            to: fuxi_core::task::TaskState::Cancelled,
                            ..
                        }
                    ) && ev.meta.task == Some(task_id)
                    {
                        found = true;
                        break;
                    }
                }
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => continue,
            }
        }
        assert!(
            found,
            "pump 无终态退出时应兜底发 TaskStateChanged{{Cancelled}}"
        );
    }

    /// Bug 修（issue 8ebff743）：terminal 后的 wind-down 事件（`UsageReport` /
    /// `ThinkingFinished`）不得把 `saw_terminal` 重置。cc `ResultSuccess` 翻译产物
    /// 就是「`TaskStateChanged{Done}` + 紧跟 `UsageReport`（cc result 几乎必带
    /// usage）+ `ThinkingFinished`」。旧逻辑把这些同 turn 收尾事件误判成「新 turn
    /// 启动」→ pump 退回无超时 `rx.recv()` 阻塞。cc 进程持久 WS 仍活、rx 永不关闭
    /// → pump 永久卡死 → shelf 锁死 `Busy` → idle GC 的 `idle_longer_than`（只看
    /// `Idle`）永不命中 → 门客无限累积（home 实测 54 worker / 17GB RSS）。
    #[tokio::test]
    async fn pump_exits_when_terminal_followed_by_winddown_events() {
        let fuxi = make_fuxi().await;

        // sender 全程留着——模拟 cc 进程跑完 turn 后仍活（persistent WS idle）。
        let (agent, tx) = ControllableAgent::new("cangjie");
        let agent_id = fuxi.insert_agent(agent.clone(), None).await;

        let task = Task::new("extract", "noop");
        let task_id = task.id;
        fuxi.dispatch(agent_id, task).await.expect("dispatch");

        // dispatch 立即把 shelf 置 Busy
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            fuxi.status_of(agent_id).await,
            Some(ShelfStatus::Busy),
            "dispatch 后 shelf 应为 Busy"
        );

        let mk = |kind: EventKind| {
            let mut m = EventMeta::now();
            m.agent = Some(agent_id);
            m.task = Some(task_id);
            Event { meta: m, kind }
        };
        // cc ResultSuccess 翻译产物的真实顺序：terminal 先发，wind-down 紧跟。
        tx.send(mk(EventKind::TaskStateChanged {
            from: fuxi_core::task::TaskState::Delivering,
            to: fuxi_core::task::TaskState::Done,
        }))
        .await
        .unwrap();
        tx.send(mk(EventKind::UsageReport {
            input_tokens: 100,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 50,
            total_tokens: 150,
            window_size: 200_000,
            pct: 0.001,
        }))
        .await
        .unwrap();
        tx.send(mk(EventKind::ThinkingFinished)).await.unwrap();

        // pump 看到 terminal + grace 窗口内只剩 wind-down → 应退出、摊回 Idle。
        let mut idle = false;
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if fuxi.status_of(agent_id).await == Some(ShelfStatus::Idle) {
                idle = true;
                break;
            }
        }
        // tx 始终持有——若仍 Busy 即证明 pump 卡死在 rx.recv()。
        drop(tx);
        assert!(
            idle,
            "terminal 后只剩 wind-down 事件时 pump 应退出、shelf 摊回 Idle"
        );
    }

    // ── v2 跨节点：auto-pin from project.host_nodes ─────────────────────

    use crate::node_load::{NodeLoadProvider, NodeLoadSnapshot};
    use fuxi_workspace::FileSystemProjectRegistry;

    struct StubLoadProvider {
        snaps: Vec<NodeLoadSnapshot>,
    }
    #[async_trait]
    impl NodeLoadProvider for StubLoadProvider {
        async fn snapshot(&self) -> Vec<NodeLoadSnapshot> {
            self.snaps.clone()
        }
    }

    /// 在 tempdir 起一个最小 git repo 给 registry.add 通过 NotAGitRepo 校验。
    async fn make_seed_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await;
        }
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "x"])
            .output()
            .await;
        (dir, path)
    }

    /// 仅 1 个 host_node + 任 task 无 pinned_node → auto-pin 必命中那个唯一节点。
    #[tokio::test]
    async fn auto_pin_picks_only_host_node_when_single() {
        let fuxi = make_fuxi().await;
        let registry_root = tempfile::tempdir().unwrap();
        let registry = Arc::new(FileSystemProjectRegistry::new(registry_root.path()));
        let (_repo_td, repo) = make_seed_repo().await;
        registry.add(repo, Some("erp".into()), None).await.unwrap();
        registry
            .add_host_node(&fuxi_core::ProjectId::new("erp").unwrap(), "home")
            .await
            .unwrap();
        fuxi.set_project_registry(registry).await;

        // home 在线
        fuxi.set_node_load_provider(Arc::new(StubLoadProvider {
            snaps: vec![NodeLoadSnapshot {
                node_id: "home".into(),
                inflight: 0,
                max_concurrency: 4,
                online: true,
            }],
        }))
        .await;

        let task = Task::new("t", "").with_project_id(fuxi_core::ProjectId::new("erp").unwrap());
        let pinned = fuxi.auto_pin_from_project(&task).await;
        assert_eq!(pinned, Some("home".to_string()));
    }

    /// 多 host_node + saturation 不同 → 选最闲。
    #[tokio::test]
    async fn auto_pin_picks_least_loaded() {
        let fuxi = make_fuxi().await;
        let registry_root = tempfile::tempdir().unwrap();
        let registry = Arc::new(FileSystemProjectRegistry::new(registry_root.path()));
        let (_repo_td, repo) = make_seed_repo().await;
        registry.add(repo, Some("erp".into()), None).await.unwrap();
        let pid = fuxi_core::ProjectId::new("erp").unwrap();
        registry.add_host_node(&pid, "home").await.unwrap();
        registry.add_host_node(&pid, "mac").await.unwrap();
        fuxi.set_project_registry(registry).await;

        // home 满 / mac 空闲 → 选 mac
        fuxi.set_node_load_provider(Arc::new(StubLoadProvider {
            snaps: vec![
                NodeLoadSnapshot {
                    node_id: "home".into(),
                    inflight: 4,
                    max_concurrency: 4,
                    online: true,
                },
                NodeLoadSnapshot {
                    node_id: "mac".into(),
                    inflight: 0,
                    max_concurrency: 4,
                    online: true,
                },
            ],
        }))
        .await;

        let task = Task::new("t", "").with_project_id(pid);
        let pinned = fuxi.auto_pin_from_project(&task).await;
        assert_eq!(pinned, Some("mac".to_string()));
    }

    /// 候选全离线 → 返 None（caller 决定 fallback 策略）。
    #[tokio::test]
    async fn auto_pin_none_when_all_candidates_offline() {
        let fuxi = make_fuxi().await;
        let registry_root = tempfile::tempdir().unwrap();
        let registry = Arc::new(FileSystemProjectRegistry::new(registry_root.path()));
        let (_repo_td, repo) = make_seed_repo().await;
        registry.add(repo, Some("erp".into()), None).await.unwrap();
        let pid = fuxi_core::ProjectId::new("erp").unwrap();
        registry.add_host_node(&pid, "mac").await.unwrap();
        fuxi.set_project_registry(registry).await;

        fuxi.set_node_load_provider(Arc::new(StubLoadProvider {
            snaps: vec![NodeLoadSnapshot {
                node_id: "mac".into(),
                inflight: 0,
                max_concurrency: 4,
                online: false,
            }],
        }))
        .await;

        let task = Task::new("t", "").with_project_id(pid);
        let pinned = fuxi.auto_pin_from_project(&task).await;
        assert!(pinned.is_none());
    }

    /// host_nodes 空（v1 单节点项目）→ 不 auto-pin（保留旧行为）。
    #[tokio::test]
    async fn auto_pin_none_when_host_nodes_empty() {
        let fuxi = make_fuxi().await;
        let registry_root = tempfile::tempdir().unwrap();
        let registry = Arc::new(FileSystemProjectRegistry::new(registry_root.path()));
        let (_repo_td, repo) = make_seed_repo().await;
        registry.add(repo, Some("erp".into()), None).await.unwrap();
        let pid = fuxi_core::ProjectId::new("erp").unwrap();
        // 不调 add_host_node
        fuxi.set_project_registry(registry).await;
        fuxi.set_node_load_provider(Arc::new(StubLoadProvider { snaps: vec![] }))
            .await;

        let task = Task::new("t", "").with_project_id(pid);
        let pinned = fuxi.auto_pin_from_project(&task).await;
        assert!(pinned.is_none());
    }

    /// 用户已显式 pin → auto_pin 不该覆盖（caller 端用 task.pinned_node.is_none() 守卫；
    /// 但单测对 auto_pin 接口本身——传入 task.pinned_node = Some(...) 时也应返 None
    /// 让 caller 不至于二次写）。
    #[tokio::test]
    async fn auto_pin_respects_existing_pinned_node() {
        let fuxi = make_fuxi().await;
        let registry_root = tempfile::tempdir().unwrap();
        let registry = Arc::new(FileSystemProjectRegistry::new(registry_root.path()));
        let (_repo_td, repo) = make_seed_repo().await;
        registry.add(repo, Some("erp".into()), None).await.unwrap();
        let pid = fuxi_core::ProjectId::new("erp").unwrap();
        registry.add_host_node(&pid, "home").await.unwrap();
        fuxi.set_project_registry(registry).await;
        fuxi.set_node_load_provider(Arc::new(StubLoadProvider {
            snaps: vec![NodeLoadSnapshot {
                node_id: "home".into(),
                inflight: 0,
                max_concurrency: 4,
                online: true,
            }],
        }))
        .await;

        let task = Task::new("t", "")
            .with_project_id(pid)
            .with_pinned_node("explicit-mac");
        let pinned = fuxi.auto_pin_from_project(&task).await;
        assert!(
            pinned.is_none(),
            "已 pin 的 task 不应 auto-pin（避免覆盖用户意图）"
        );
    }

    /// pump 见到正常 Done 终态 → 不应额外 emit TaskCancelled（避免事件污染）。
    #[tokio::test]
    async fn pump_does_not_double_emit_when_terminal_seen() {
        let fuxi = make_fuxi().await;
        let bus = fuxi.bus();
        let mut sub = bus.subscribe();

        let (agent, tx) = ControllableAgent::new("luban");
        let _agent_id = fuxi.insert_agent(agent.clone(), None).await;
        let task = Task::new("test-task-done", "noop");
        let task_id = task.id;
        fuxi.dispatch(agent.card().id, task)
            .await
            .expect("dispatch");

        // 给 pump 起来时间
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // pump 内部已发 TaskCreated/TaskDispatched；这里 push 一条真终态 Done
        let mut meta = EventMeta::now();
        meta.agent = Some(agent.card().id);
        meta.task = Some(task_id);
        tx.send(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .await
        .expect("push done");

        // 等 grace timeout 让 pump 退出（默认 500ms）
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        drop(tx);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 数 Cancelled 事件——应当 0 条（task 已 Done）
        let mut cancelled_count = 0;
        for _ in 0..30 {
            match tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await {
                Ok(Some(Ok(ev))) => {
                    if matches!(
                        &ev.kind,
                        EventKind::TaskStateChanged {
                            to: fuxi_core::task::TaskState::Cancelled,
                            ..
                        }
                    ) && ev.meta.task == Some(task_id)
                    {
                        cancelled_count += 1;
                    }
                }
                _ => break,
            }
        }
        assert_eq!(
            cancelled_count, 0,
            "saw_terminal=true 时不应再补 TaskCancelled（避免重复终态）"
        );
    }

    /// FU-2 收尾（2026-06-10）：always-nudge fallback 的 `AgentRequestReview` 也要带
    /// 发起 task 的 `topic_id`，否则门客完工兜底信号 meta.topic_id 为空——bridge 虽按
    /// task.topic_id 解析路由仍对，但事件元数据不全，订阅方按 topic 过滤会漏。
    #[tokio::test]
    async fn always_nudge_stamps_task_topic_id() {
        let fuxi = make_fuxi().await;
        let bus = fuxi.bus();
        let mut sub = bus.subscribe();

        let topic = fuxi_core::TopicId::new();
        let (agent, tx) = ControllableAgent::new("luban");
        fuxi.insert_agent(agent.clone(), None).await;
        let task = Task::new("topic-nudge", "noop").with_topic_id(topic);
        let task_id = task.id;
        fuxi.dispatch(agent.card().id, task)
            .await
            .expect("dispatch");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // push 真终态 Done（无 review_request → 触发 always-nudge）。
        let mut meta = EventMeta::now();
        meta.agent = Some(agent.card().id);
        meta.task = Some(task_id);
        tx.send(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::InProgress,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .await
        .expect("push done");

        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        let mut found = None;
        for _ in 0..50 {
            match tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await {
                Ok(Some(Ok(ev))) => {
                    if matches!(&ev.kind, EventKind::AgentRequestReview { .. })
                        && ev.meta.task == Some(task_id)
                    {
                        found = Some(ev);
                        break;
                    }
                }
                _ => break,
            }
        }
        let ev = found.expect("always-nudge 应兜底发 AgentRequestReview");
        assert_eq!(
            ev.meta.topic_id,
            Some(topic),
            "always-nudge 的 AgentRequestReview 应带发起 task 的 topic_id"
        );
    }
}

#[cfg(test)]
mod dispatch_routing_tests {
    //! issue f4e0ff39：`--to <本地 agent>` + `--required-tags` 不该卡 dist queue。
    //! home 既是 controller 又是唯一 worker、且**无 pull loop**，task 进 dist queue
    //! 永远无人 pull。修复：agent 在本地 shelf 存在时，required_tags 不触发 dist；
    //! 但 pinned_node 永远走 dist（玄女显式跨节点路由必须保留）。

    use super::*;
    use async_trait::async_trait;
    use fuxi_core::Result as CoreResult;
    use fuxi_core::agent::{AgentCard, AgentProfile, AgentStatus};
    use fuxi_events::EventBus;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;
    use tokio::sync::mpsc;

    struct StubAgent {
        card: AgentCard,
        rx_holder: Mutex<Option<mpsc::Receiver<Event>>>,
    }
    impl StubAgent {
        fn new(role: &str) -> (Arc<Self>, mpsc::Sender<Event>) {
            let (tx, rx) = mpsc::channel(64);
            let agent = Arc::new(Self {
                card: AgentCard {
                    id: AgentId::new(),
                    profile: AgentProfile {
                        name: format!("stub-{role}"),
                        role: role.to_string(),
                        cli: "stub".to_string(),
                        system_prompt: String::new(),
                        tags: vec![],
                        extra: Default::default(),
                    },
                    endpoint: "stub://".into(),
                    status: AgentStatus::Idle,
                },
                rx_holder: Mutex::new(Some(rx)),
            });
            (agent, tx)
        }
    }
    #[async_trait]
    impl Agent for StubAgent {
        fn card(&self) -> &AgentCard {
            &self.card
        }
        async fn dispatch(&self, _task: Task) -> CoreResult<mpsc::Receiver<Event>> {
            Ok(self.rx_holder.lock().await.take().expect("rx 已取走"))
        }
        async fn send_message(&self, _t: TaskId, _text: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn cancel(&self, _t: TaskId) -> CoreResult<()> {
            Ok(())
        }
        async fn shutdown(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    /// 记录 enqueue 调用次数的 mock——验证 dist 路径是否被触发。
    struct CountingEnqueuer {
        count: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl crate::DistEnqueuer for CountingEnqueuer {
        async fn enqueue(&self, _task: &Task, _opts: crate::DistEnqueueOptions) -> Result<String> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok("job-stub".to_string())
        }
    }

    async fn make_fuxi() -> Arc<Fuxi> {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let bus = EventBus::with_memory_store().await.unwrap();
        std::mem::forget(dir);
        Arc::new(Fuxi::new(bus, ws))
    }

    /// 核心修复：`--to <本地 agent>` + `--required-tags home` 走本地直派，不 enqueue。
    #[tokio::test]
    async fn required_tags_with_local_agent_dispatches_locally_not_dist() {
        let fuxi = make_fuxi().await;
        let count = Arc::new(AtomicUsize::new(0));
        fuxi.set_dist_enqueuer(Arc::new(CountingEnqueuer {
            count: count.clone(),
        }))
        .await;

        let (agent, _tx) = StubAgent::new("luban");
        let agent_id = fuxi.insert_agent(agent.clone(), None).await;

        let task =
            Task::new("home-maint", "重启 sovits").with_required_tags(vec!["home".to_string()]);
        fuxi.dispatch(agent_id, task).await.expect("dispatch");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "本地 agent + required_tags 应直派本地，不进 dist queue"
        );
        assert_eq!(
            fuxi.status_of(agent_id).await,
            Some(ShelfStatus::Busy),
            "本地直派后 agent 应 Busy（被本地 pump 接管）"
        );
    }

    /// pinned_node 永远走 dist——保护玄女显式跨节点路由不被本修复破坏。
    #[tokio::test]
    async fn pinned_node_with_local_agent_still_goes_dist() {
        let fuxi = make_fuxi().await;
        let count = Arc::new(AtomicUsize::new(0));
        fuxi.set_dist_enqueuer(Arc::new(CountingEnqueuer {
            count: count.clone(),
        }))
        .await;

        let (agent, _tx) = StubAgent::new("luban");
        let agent_id = fuxi.insert_agent(agent.clone(), None).await;

        let task = Task::new("on-mac", "跑 mac 专属活").with_pinned_node("mac-studio".to_string());
        fuxi.dispatch(agent_id, task).await.expect("dispatch");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "pinned_node 显式跨节点应走 dist enqueue，无论 agent 是否本地"
        );
    }

    /// required_tags + agent 不在本地 shelf → 仍走 dist（跨节点 role 路由保留）。
    #[tokio::test]
    async fn required_tags_with_nonlocal_agent_goes_dist() {
        let fuxi = make_fuxi().await;
        let count = Arc::new(AtomicUsize::new(0));
        fuxi.set_dist_enqueuer(Arc::new(CountingEnqueuer {
            count: count.clone(),
        }))
        .await;

        // 不 insert_agent——agent_id 不在本地 shelf
        let ghost = AgentId::new();
        let task = Task::new("remote", "远端活").with_required_tags(vec!["erp".to_string()]);
        fuxi.dispatch(ghost, task).await.expect("dispatch");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "agent 不在本地 + required_tags → 走 dist enqueue"
        );
    }
}

#[cfg(test)]
mod xuannv_pool_integration_tests {
    //! 块2.2/2.3：Fuxi 持 XuannvPool 后的 topic 维度 API + 豁免改造。

    use super::*;
    use async_trait::async_trait;
    use fuxi_core::Result as CoreResult;
    use fuxi_core::TopicId;
    use fuxi_core::agent::{AgentCard, AgentProfile, AgentStatus};
    use fuxi_events::EventBus;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// 最小 agent stub——只为占 shelf 一个 id，dispatch 不跑。
    struct NullAgent {
        card: AgentCard,
    }
    impl NullAgent {
        fn new(role: &str) -> Arc<Self> {
            Arc::new(Self {
                card: AgentCard {
                    id: AgentId::new(),
                    profile: AgentProfile {
                        name: format!("null-{role}"),
                        role: role.into(),
                        cli: "stub".into(),
                        system_prompt: String::new(),
                        tags: vec![],
                        extra: Default::default(),
                    },
                    endpoint: "stub://".into(),
                    status: AgentStatus::Idle,
                },
            })
        }
    }
    #[async_trait]
    impl Agent for NullAgent {
        fn card(&self) -> &AgentCard {
            &self.card
        }
        async fn dispatch(&self, _task: Task) -> CoreResult<mpsc::Receiver<Event>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
        async fn send_message(&self, _t: TaskId, _text: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn cancel(&self, _t: TaskId) -> CoreResult<()> {
            Ok(())
        }
        async fn shutdown(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    async fn make_fuxi() -> Arc<Fuxi> {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let bus = EventBus::with_memory_store().await.unwrap();
        std::mem::forget(dir);
        Arc::new(Fuxi::new(bus, ws))
    }

    /// Task 2.2：set_xuannv_for_topic / xuannv_id_for_topic 走池往返。
    #[tokio::test]
    async fn fuxi_xuannv_id_for_topic_follows_pool() {
        let fuxi = make_fuxi().await;
        let topic = TopicId(uuid::Uuid::nil());
        let id = AgentId::new();
        fuxi.set_xuannv_for_topic(topic, id).await;
        assert_eq!(fuxi.xuannv_id_for_topic(topic).await, Some(id));
        // 未绑定的 topic 返回 None。
        assert_eq!(fuxi.xuannv_id_for_topic(TopicId::new()).await, None);
    }

    /// Task 2.2 兼容壳：set_xuannv(id) 等价于绑 general topic；xuannv_id() 读回它。
    #[tokio::test]
    async fn legacy_set_xuannv_routes_to_general_topic() {
        let fuxi = make_fuxi().await;
        let id = AgentId::new();
        fuxi.set_xuannv(id).await;
        assert_eq!(fuxi.xuannv_id().await, Some(id));
        assert_eq!(
            fuxi.xuannv_id_for_topic(TopicId::general()).await,
            Some(id),
            "set_xuannv 应绑到 general topic"
        );
    }

    /// Task 2.2 兼容壳：xuannv_id_watch 仍跟随 general 分身漂移（idle_gc 豁免靠它）。
    #[tokio::test]
    async fn legacy_xuannv_id_watch_follows_general_clone() {
        let fuxi = make_fuxi().await;
        let mut rx = fuxi.xuannv_id_watch();
        let id = AgentId::new();
        fuxi.set_xuannv_for_topic(TopicId::general(), id).await;
        // changed 后 borrow 拿到新值。
        assert!(rx.changed().await.is_ok());
        assert_eq!(*rx.borrow_and_update(), Some(id));
    }

    /// Task 2.3：shutdown_agent 命中池中任一活分身（非 general 也算）→ 拒绝永久 kill。
    #[tokio::test]
    async fn shutdown_agent_exempts_any_active_clone() {
        let fuxi = make_fuxi().await;
        // 把一个分身 id 同时放进 shelf 和池（非 general topic）。
        let clone_agent = NullAgent::new("xuannv") as Arc<dyn Agent>;
        let clone_id = clone_agent.card().id;
        fuxi.insert_agent(clone_agent, None).await;
        let topic = TopicId::new();
        fuxi.set_xuannv_for_topic(topic, clone_id).await;

        // shutdown_agent 应豁免：返回 Ok 且 shelf 仍持有该分身（没被 take 走）。
        fuxi.shutdown_agent(clone_id, "idle_ttl".into())
            .await
            .expect("豁免应返回 Ok noop");
        assert!(
            fuxi.status_of(clone_id).await.is_some(),
            "活分身不该被 shutdown_agent 永久 kill"
        );
    }

    /// 记录被 dispatch 的 task（验 topic_id 透传）。idle 状态让 intervene 走
    /// degrade-dispatch 分支。
    struct RecordingAgent {
        card: AgentCard,
        last_task: Arc<tokio::sync::Mutex<Option<Task>>>,
    }
    impl RecordingAgent {
        fn new(role: &str) -> (Arc<Self>, Arc<tokio::sync::Mutex<Option<Task>>>) {
            let slot = Arc::new(tokio::sync::Mutex::new(None));
            let agent = Arc::new(Self {
                card: AgentCard {
                    id: AgentId::new(),
                    profile: AgentProfile {
                        name: format!("rec-{role}"),
                        role: role.into(),
                        cli: "stub".into(),
                        system_prompt: String::new(),
                        tags: vec![],
                        extra: Default::default(),
                    },
                    endpoint: "stub://".into(),
                    status: AgentStatus::Idle,
                },
                last_task: slot.clone(),
            });
            (agent, slot)
        }
    }
    #[async_trait]
    impl Agent for RecordingAgent {
        fn card(&self) -> &AgentCard {
            &self.card
        }
        async fn dispatch(&self, task: Task) -> CoreResult<mpsc::Receiver<Event>> {
            *self.last_task.lock().await = Some(task);
            // 立即关闭 rx（无终态）——dispatch pump 会兜底 TaskCancelled，本测试不关心。
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
        async fn send_message(&self, _t: TaskId, _text: &str) -> CoreResult<()> {
            Ok(())
        }
        async fn cancel(&self, _t: TaskId) -> CoreResult<()> {
            Ok(())
        }
        async fn shutdown(&self) -> CoreResult<()> {
            Ok(())
        }
    }

    /// Task 3.1：玄女分身（服务 topic A）idle-degrade 出的 user-turn task，
    /// 其 `topic_id == Some(A)`——让这轮对话事件归位该 topic。
    #[tokio::test]
    async fn clone_degrade_dispatch_stamps_owning_topic() {
        let fuxi = make_fuxi().await;
        let topic_a = TopicId::new();
        // 把 RecordingAgent 当作 topic A 的玄女分身：既在 shelf（idle）又在池。
        let (clone_agent, slot) = RecordingAgent::new("xuannv");
        let clone_id = clone_agent.card().id;
        fuxi.insert_agent(clone_agent, None).await;
        fuxi.set_xuannv_for_topic(topic_a, clone_id).await;

        // 对该分身 intervene（idle → degrade dispatch）。
        fuxi.intervene(clone_id, false, "继续画头像", vec![], None, vec![], None)
            .await
            .expect("intervene 应 degrade-dispatch 成功");

        let task = slot
            .lock()
            .await
            .clone()
            .expect("应捕获到被 dispatch 的 task");
        assert_eq!(
            task.topic_id,
            Some(topic_a),
            "分身的 user-turn task 应打上她服务的 topic"
        );
    }

    /// Task 3.1 边界：intervene 目标是**普通门客**（不在池）→ degrade task 不挂
    /// topic（发起方 topic 此入口拿不到，留块5 补 spawn env）。不能误挂别的 topic。
    #[tokio::test]
    async fn worker_degrade_dispatch_leaves_topic_none() {
        let fuxi = make_fuxi().await;
        // 池里放一个别的 topic 的分身，确认不会被误用到 worker task 上。
        fuxi.set_xuannv_for_topic(TopicId::new(), AgentId::new())
            .await;

        let (worker_agent, slot) = RecordingAgent::new("luban");
        let worker_id = worker_agent.card().id;
        fuxi.insert_agent(worker_agent, None).await;

        fuxi.intervene(worker_id, false, "改个 bug", vec![], None, vec![], None)
            .await
            .expect("intervene 应 degrade-dispatch 成功");

        let task = slot
            .lock()
            .await
            .clone()
            .expect("应捕获到被 dispatch 的 task");
        assert_eq!(
            task.topic_id, None,
            "普通门客 degrade task 不该挂任何 topic（块5 补发起方 topic）"
        );
    }

    /// 块5 mock spawner：记录被请求 spawn 的 topic，并模拟 adapter 真实行为
    /// （set_xuannv_for_topic 入池后返回新 id）。
    struct MockSpawner {
        fuxi: std::sync::Mutex<Option<std::sync::Weak<Fuxi>>>,
        spawned: Arc<tokio::sync::Mutex<Vec<TopicId>>>,
    }
    #[async_trait]
    impl crate::XuannvSpawner for MockSpawner {
        async fn spawn_for_topic(&self, topic: TopicId) -> crate::Result<AgentId> {
            self.spawned.lock().await.push(topic);
            let id = AgentId::new();
            // 模拟 adapter：spawn 后入池（真 adapter 走 spawn_with_prelude → set_xuannv_for_topic）。
            let weak = self.fuxi.lock().unwrap().clone();
            if let Some(fuxi) = weak.and_then(|w| w.upgrade()) {
                fuxi.set_xuannv_for_topic(topic, id).await;
            }
            Ok(id)
        }
    }

    /// Task 7.1：ensure_xuannv_for_topic 池有活分身 → 直接返回，**不** spawn。
    #[tokio::test]
    async fn ensure_xuannv_for_topic_returns_existing_without_spawn() {
        let fuxi = make_fuxi().await;
        let topic = TopicId::new();
        let existing = AgentId::new();
        fuxi.set_xuannv_for_topic(topic, existing).await;

        let spawned = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        fuxi.set_xuannv_spawner(Arc::new(MockSpawner {
            fuxi: std::sync::Mutex::new(Some(Arc::downgrade(&fuxi))),
            spawned: spawned.clone(),
        }))
        .await;

        let got = fuxi.ensure_xuannv_for_topic(topic).await;
        assert_eq!(got, Some(existing), "池有活分身应直接返回");
        assert!(spawned.lock().await.is_empty(), "不该触发 spawn");
    }

    /// Task 7.1：ensure_xuannv_for_topic 池 miss → 调 spawner spawn 一只并入池返回。
    #[tokio::test]
    async fn ensure_xuannv_for_topic_spawns_on_miss() {
        let fuxi = make_fuxi().await;
        let topic = TopicId::new();

        let spawned = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        fuxi.set_xuannv_spawner(Arc::new(MockSpawner {
            fuxi: std::sync::Mutex::new(Some(Arc::downgrade(&fuxi))),
            spawned: spawned.clone(),
        }))
        .await;

        let got = fuxi.ensure_xuannv_for_topic(topic).await;
        assert!(got.is_some(), "池 miss + 有 spawner 应 spawn 出新分身");
        assert_eq!(
            spawned.lock().await.as_slice(),
            &[topic],
            "spawner 应按该 topic 调一次"
        );
        // 入池了：再 ensure 直接命中、不再 spawn。
        let again = fuxi.ensure_xuannv_for_topic(topic).await;
        assert_eq!(again, got, "spawn 后入池，再 ensure 命中同一只");
        assert_eq!(spawned.lock().await.len(), 1, "第二次 ensure 不该再 spawn");
    }

    /// Task 7.1：池 miss + 未注入 spawner（兼容期/测试）→ 返回 None，不 panic。
    #[tokio::test]
    async fn ensure_xuannv_for_topic_none_without_spawner() {
        let fuxi = make_fuxi().await;
        let got = fuxi.ensure_xuannv_for_topic(TopicId::new()).await;
        assert_eq!(got, None, "无 spawner 池 miss 应返 None");
    }

    /// 等 general 镜像 reconciler 把 xuannv_id() 收敛到期望值（异步传播，给点时间）。
    async fn wait_xuannv_id(fuxi: &Fuxi, want: Option<AgentId>) -> bool {
        for _ in 0..100 {
            if fuxi.xuannv_id().await == want {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        false
    }

    /// 块5 硬回归（守今天的「玄女假活黑洞」）：general 分身被 dormant 回收后，下一条
    /// 用户消息触发 respawn——新 id 入池 + xuannv_id 镜像更新到新 id，不打到死 id。
    #[tokio::test]
    async fn general_clone_dormant_reaped_then_user_message_respawns_not_blackhole() {
        let fuxi = make_fuxi().await;
        let general = TopicId::general();

        // 1. 起始：general 分身在池（bootstrap set_xuannv 等价）。
        let old = AgentId::new();
        fuxi.set_xuannv_for_topic(general, old).await;
        assert!(wait_xuannv_id(&fuxi, Some(old)).await, "镜像应同步到 old");

        let spawned = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        fuxi.set_xuannv_spawner(Arc::new(MockSpawner {
            fuxi: std::sync::Mutex::new(Some(Arc::downgrade(&fuxi))),
            spawned: spawned.clone(),
        }))
        .await;

        // 2. 模拟 idle_gc dormant 回收 general（pool.remove，不碰镜像）。
        fuxi.xuannv_pool().remove(general).await;
        // reconciler 必须把镜像清成 None——否则就是黑洞（xuannv_id 还指死 old）。
        assert!(
            wait_xuannv_id(&fuxi, None).await,
            "dormant 回收 general 后镜像必须清 None（不留死 id 黑洞）"
        );

        // 3. 下一条用户消息走 ensure_xuannv_for_topic(general) → respawn。
        let new = fuxi
            .ensure_xuannv_for_topic(general)
            .await
            .expect("respawn 应起出新分身");
        assert_ne!(new, old, "应是全新分身 id，不是复活死 id");
        assert_eq!(
            spawned.lock().await.as_slice(),
            &[general],
            "spawner 按 general 调一次"
        );
        // 4. 新 id 入池 + 镜像更新到新 id（用户消息打到活分身，不黑洞）。
        assert_eq!(fuxi.xuannv_id_for_topic(general).await, Some(new));
        assert!(
            wait_xuannv_id(&fuxi, Some(new)).await,
            "镜像应更新到 respawn 的新 id"
        );
    }
}
