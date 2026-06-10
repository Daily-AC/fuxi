//! 门客 idle GC —— M2.4 D4 · 空闲超时回收。
//!
//! 现象：cc 是 persistent WS，idle 门客不主动退；堆几十个后资源吃光。
//!
//! 设计：
//! - [`IdleGcTask`] 每 `tick_interval`（默认 30s）扫一次 [`crate::Shelf`]。
//! - 超过 `ttl`（默认 `FUXI_IDLE_TTL_SECS` 或 600s）没动的 idle 门客 → 发
//!   `AgentShuttingDown { reason: "idle_ttl" }`（激活 publisher-orphan）→ 调
//!   [`IdleShutdowner::shutdown_idle`] 走 orchestrator 层清理（agent.shutdown +
//!   worktree.destroy + AgentDead）。
//! - 依赖方向：本模块住在 orchestrator 内，直接借用 `Arc<Shelf>` 读超时列表；
//!   scheduler 不 depend orchestrator，所以 tick 机制落地在这里。
//!
//! 公理：
//! - #3（真实时，不轮询）—— 本 tick 是"回收扫描"，不是业务轮询；idle 状态本身由
//!   dispatch pump 事件驱动，GC 只读 [`Shelf::idle_longer_than`] 快照。
//! - #1（headless agent 不显式沟通 = 没做）—— GC 决策必经 EventBus，不私下清理。
//!
//! TTL 从 env `FUXI_IDLE_TTL_SECS` 读；parse 失败 / 缺省用 600s。tick 固定 30s
//! 不做 env override（太细的 knob 暂无必要）。

use crate::error::Result;
use crate::registry::Shelf;
use async_trait::async_trait;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::AgentId;
use fuxi_events::EventBus;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// GC 默认 TTL：10 分钟。roadmap §M2.4 定的经验值，和 `FUXI_IDLE_TTL_SECS` env
/// 的默认取值统一。
pub const DEFAULT_IDLE_TTL_SECS: u64 = 600;
/// GC 默认 tick 间隔：30s。扫描成本极低（只读 Shelf），太快无意义。
pub const DEFAULT_TICK_INTERVAL_SECS: u64 = 30;

/// 读 `FUXI_IDLE_TTL_SECS` env；非法 / 缺省走 `DEFAULT_IDLE_TTL_SECS`。
///
/// WHY 抽独立函数：repl.rs 启动时取 TTL 构造 `IdleGcTask`；测试可走 `tick_once`
/// 传自定义 ttl 绕过 env。
pub fn ttl_from_env() -> Duration {
    let secs = std::env::var("FUXI_IDLE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_TTL_SECS);
    Duration::from_secs(secs)
}

/// 实际做 shutdown 的 handle——把 GC 和 Fuxi 之间做 trait 隔离，好 mock 测试。
///
/// 生产用 `impl IdleShutdowner for Arc<Fuxi>`（见 `fuxi.rs` 末尾），直接走
/// `Fuxi::shutdown_agent`；单测可以塞个计数 stub。
#[async_trait]
pub trait IdleShutdowner: Send + Sync {
    /// 对指定 id 执行关停。语义同 `Fuxi::shutdown_agent`：幂等、事件自带 reason。
    async fn shutdown_idle(&self, id: AgentId, reason: String) -> Result<()>;
}

/// idle GC 后台任务——`spawn` 起 tokio 任务，返回 `JoinHandle` 供 abort。
pub struct IdleGcTask {
    shelf: Arc<Shelf>,
    shutdowner: Arc<dyn IdleShutdowner>,
    bus: EventBus,
    ttl: Duration,
    tick_interval: Duration,
    /// Bug 修：xuannv id 的 watch handle —— GC 跳过她，避免 shutdown 风暴。
    /// 根因：shutdown_agent 对 xuannv silent return Ok（exempt），但 GC pre-publish
    /// AgentShuttingDown 事件不知道；下次 tick 再循环。8/8 storm agent 全是 xuannv，
    /// 单实例触发 1830 噪音事件（事件库实证）。
    /// 用 watch 而非裸 Option：GC 启动早于 xuannv spawn（im.rs 顺序），且 xuannv
    /// 重生后 id 会变；watch 让 tick_once 每次都 borrow 到当前真值。None = 测试。
    xuannv_id: Option<tokio::sync::watch::Receiver<Option<AgentId>>>,
    /// 块2 玄女分身池——注入后 GC 对**任一活分身**走 dormant 回收：先
    /// `pool.remove(topic)` 摘掉映射，再正常 `shutdown_idle`（进程回收 + AgentDead
    /// idle_ttl），分身后续可 respawn。未注入（None）时退回 `xuannv_id` 单豁免
    /// （旧单玄女语义，过渡期 / 测试用）。
    xuannv_pool: Option<Arc<crate::xuannv_pool::XuannvPool>>,
}

impl IdleGcTask {
    /// 构造。TTL 一般走 [`ttl_from_env`]；tick_interval 走默认 30s。
    /// xuannv_id 默认 None；构造后用 [`Self::with_xuannv_exempt`] 链式注入。
    pub fn new(
        shelf: Arc<Shelf>,
        shutdowner: Arc<dyn IdleShutdowner>,
        bus: EventBus,
        ttl: Duration,
        tick_interval: Duration,
    ) -> Self {
        Self {
            shelf,
            shutdowner,
            bus,
            ttl,
            tick_interval,
            xuannv_id: None,
            xuannv_pool: None,
        }
    }

    /// 注入 xuannv id watch handle —— GC tick_once 跳过她，不预发 AgentShuttingDown。
    /// 生产用 `fuxi.xuannv_id_watch()`；测试可手 build：
    /// `let (_, rx) = tokio::sync::watch::channel(Some(id));`
    ///
    /// 块2 后这是 **fallback**：未注入 [`Self::with_xuannv_pool`] 时才生效（旧单玄女
    /// 语义）。注入池后分身走 dormant 回收，不再 skip。
    pub fn with_xuannv_exempt(
        mut self,
        watch: tokio::sync::watch::Receiver<Option<AgentId>>,
    ) -> Self {
        self.xuannv_id = Some(watch);
        self
    }

    /// 块2：注入玄女分身池——GC 对任一活分身走 dormant 回收（pool.remove + 正常
    /// shutdown_idle），分身后续可 respawn。生产用 `fuxi.xuannv_pool()`。
    pub fn with_xuannv_pool(mut self, pool: Arc<crate::xuannv_pool::XuannvPool>) -> Self {
        self.xuannv_pool = Some(pool);
        self
    }

    /// 起后台 loop。`JoinHandle::abort()` 可立即停（shutdown path 用）。
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                ttl_secs = self.ttl.as_secs(),
                tick_secs = self.tick_interval.as_secs(),
                "idle GC: 启动"
            );
            let mut ticker = tokio::time::interval(self.tick_interval);
            // interval 首次 tick 立即 ready——跳掉，让第一次真正扫发生在 tick_interval 之后
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(e) = self.tick_once().await {
                    warn!(error = %e, "idle GC tick 出错，继续下轮");
                }
            }
        })
    }

    /// 单次扫——供测试直接驱动，绕开 interval。
    ///
    /// 任何子步骤失败只 warn，不 panic 不传播——GC 必须尽量撑到下一 tick。
    pub async fn tick_once(&self) -> Result<usize> {
        let stale = self.shelf.idle_longer_than(self.ttl).await;
        if stale.is_empty() {
            return Ok(0);
        }
        debug!(count = stale.len(), "idle GC: 发现过期门客");
        let mut count = 0usize;
        // 取当前 xuannv id 一次（watch borrow 廉价，但避免在 for 内每次 borrow）。
        let xuannv_now = self.xuannv_id.as_ref().and_then(|w| *w.borrow());
        for id in stale {
            // 块2：池模式优先。命中池中活分身 → dormant 回收：先 pool.remove(topic)
            // 摘掉映射（之后 shutdown_agent 的 is_active_clone 豁免不再拦），再走
            // 正常 shutdown_idle（进程回收 + AgentDead idle_ttl），分身后续可 respawn。
            // 不 skip——dormant 正是本特性的目的（空闲分身释放资源，下条消息再起）。
            if let Some(pool) = self.xuannv_pool.as_ref() {
                if let Some(topic) = pool.topic_of(id).await {
                    // 块5 修（2026-06-10 home 实测）：general 分身是常驻 home base，
                    // **永不 dormant 回收**。否则回收后 `xuannv_id` 置 None，而
                    // `ensure_xuannv` 只 boot 时调一次、无 HTTP 触发 respawn → general
                    // 玄女永久 503 到重启（CLAUDE.md 公理「IdleGcTask 必须豁免玄女」）。
                    // 非 general 的 topic 分身仍可 dormant——由 bridge 里程碑 respawn。
                    if topic == fuxi_core::TopicId::general() {
                        continue;
                    }
                    debug!(agent = %id, %topic, "idle GC: 分身超时 → dormant 回收");
                    pool.remove(topic).await;
                }
                // 落到下面正常回收路径（无论是否分身）。
            } else if xuannv_now == Some(id) {
                // Fallback（未注入池）：旧单玄女语义——xuannv 不能被 GC 关
                // （shutdown_agent silent return Ok），且旧版仍预发 AgentShuttingDown
                // → 30s 一次永远循环。这里在预发之前就跳过她，从源头消噪音。事件库
                // 实证：单 xuannv 实例触发 1830 噪音事件（agent f6e2b1a2，
                // 2026-05-04 17:29 起 15h）。
                continue;
            }
            // 先发 AgentShuttingDown——激活 publisher-orphan + 让订阅者能在
            // 实际 shutdown 完成前就知道 intent。reason 字符串是 API 契约："idle_ttl"。
            let mut meta = EventMeta::now();
            meta.agent = Some(id);
            let _ = self.bus.publish(Event {
                meta,
                kind: EventKind::AgentShuttingDown {
                    reason: "idle_ttl".into(),
                },
            });
            // 真实清理——失败不中断别的 stale，继续下一个。
            if let Err(e) = self.shutdowner.shutdown_idle(id, "idle_ttl".into()).await {
                warn!(agent = %id, error = %e, "idle GC: shutdown_idle 失败");
            } else {
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Shelf, ShelfEntry, ShelfStatus};
    use async_trait::async_trait;
    use futures_util::StreamExt;
    use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
    use fuxi_core::event::Event;
    use fuxi_core::id::TaskId;
    use fuxi_core::task::Task;
    use fuxi_core::{CoreError, Result as CoreResult};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;
    use tokio::sync::mpsc;

    /// 最小 Agent stub——本模块测试只关心 shelf / shutdowner 交互，不跑 dispatch。
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
            Err(CoreError::Other("null".into()))
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

    #[tokio::test]
    async fn shelf_tracks_idle_since_on_status_transition() {
        // Spec: Idle 设 now、Busy 清 None、Dead 清 None；新插入门客默认带 idle_since。
        let shelf = Shelf::new();
        let agent = NullAgent::new("dev") as Arc<dyn Agent>;
        let id = agent.card().id;
        shelf
            .insert(ShelfEntry {
                card: agent.card().clone(),
                agent,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now()),
            })
            .await;

        // 再走一次 set_status(Idle) —— 新时间戳 ≥ 旧时间戳（monotonic）。
        let t0 = shelf.idle_since_of(id).await.expect("初始 idle_since");
        tokio::time::sleep(Duration::from_millis(2)).await;
        shelf.set_status(id, ShelfStatus::Idle).await;
        let t1 = shelf.idle_since_of(id).await.expect("仍在 Idle");
        assert!(t1 >= t0, "Idle→Idle 应刻新 idle_since");

        // Busy 清 None
        shelf.set_status(id, ShelfStatus::Busy).await;
        assert!(
            shelf.idle_since_of(id).await.is_none(),
            "Busy 应清 idle_since"
        );

        // Dead 也清 None
        shelf.set_status(id, ShelfStatus::Idle).await;
        shelf.set_status(id, ShelfStatus::Dead).await;
        assert!(
            shelf.idle_since_of(id).await.is_none(),
            "Dead 应清 idle_since"
        );
    }

    #[tokio::test]
    async fn idle_longer_than_returns_only_stale_entries() {
        let shelf = Shelf::new();
        let old_agent = NullAgent::new("a") as Arc<dyn Agent>;
        let new_agent = NullAgent::new("b") as Arc<dyn Agent>;
        let old_id = old_agent.card().id;
        let new_id = new_agent.card().id;

        // "老" entry：100ms 前就 idle 了
        let now = Instant::now();
        shelf
            .insert(ShelfEntry {
                card: old_agent.card().clone(),
                agent: old_agent,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(now - Duration::from_millis(100)),
            })
            .await;
        // "新" entry：刚进入 idle
        shelf
            .insert(ShelfEntry {
                card: new_agent.card().clone(),
                agent: new_agent,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(now),
            })
            .await;

        let stale = shelf.idle_longer_than(Duration::from_millis(50)).await;
        assert!(stale.contains(&old_id), "100ms 前 idle 的应被列出");
        assert!(!stale.contains(&new_id), "刚 idle 的不应被列出");
    }

    // ─── GC 路径 ────────────────────────────────────────────────
    struct CountingShutdowner {
        hits: Mutex<Vec<(AgentId, String)>>,
        calls: AtomicUsize,
    }
    impl CountingShutdowner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                hits: Mutex::new(vec![]),
                calls: AtomicUsize::new(0),
            })
        }
    }
    #[async_trait]
    impl IdleShutdowner for CountingShutdowner {
        async fn shutdown_idle(&self, id: AgentId, reason: String) -> Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.hits.lock().unwrap().push((id, reason));
            Ok(())
        }
    }

    #[tokio::test]
    async fn idle_gc_tick_invokes_shutdowner_for_stale_agents() {
        let shelf = Arc::new(Shelf::new());
        let agent = NullAgent::new("dev") as Arc<dyn Agent>;
        let id = agent.card().id;
        shelf
            .insert(ShelfEntry {
                card: agent.card().clone(),
                agent,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(60)),
            })
            .await;

        let shutdowner = CountingShutdowner::new();
        let bus = EventBus::with_memory_store().await.unwrap();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner.clone(),
            bus,
            Duration::from_secs(10),
            Duration::from_secs(30),
        );
        let n = gc.tick_once().await.unwrap();
        assert_eq!(n, 1, "应回收 1 只");
        assert_eq!(shutdowner.calls.load(Ordering::Relaxed), 1);
        assert_eq!(shutdowner.hits.lock().unwrap()[0].0, id);
        assert_eq!(shutdowner.hits.lock().unwrap()[0].1, "idle_ttl");
    }

    #[tokio::test]
    async fn idle_gc_emits_agent_shutting_down_event() {
        // 断言 bus 上收到 reason=idle_ttl 的 AgentShuttingDown
        let shelf = Arc::new(Shelf::new());
        let agent = NullAgent::new("dev") as Arc<dyn Agent>;
        let id = agent.card().id;
        shelf
            .insert(ShelfEntry {
                card: agent.card().clone(),
                agent,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(60)),
            })
            .await;

        let bus = EventBus::with_memory_store().await.unwrap();
        let mut sub = bus.subscribe();
        let shutdowner = CountingShutdowner::new();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner,
            bus.clone(),
            Duration::from_secs(10),
            Duration::from_secs(30),
        );
        let _ = gc.tick_once().await.unwrap();

        // 消化事件，找 AgentShuttingDown
        let mut found = false;
        for _ in 0..8 {
            if let Ok(Some(Ok(ev))) =
                tokio::time::timeout(Duration::from_millis(100), sub.next()).await
                && let EventKind::AgentShuttingDown { reason } = &ev.kind
                && reason == "idle_ttl"
                && ev.meta.agent == Some(id)
            {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "应在 bus 上看到 AgentShuttingDown{{reason:idle_ttl}}"
        );
    }

    /// Bug 修：xuannv 即便长时间 idle 也不应被 GC 触发任何 shutdown 信号——
    /// shutdowner 不调，bus 上不发 AgentShuttingDown。
    #[tokio::test]
    async fn idle_gc_skips_xuannv_no_shutdown_no_event() {
        let shelf = Arc::new(Shelf::new());
        let xuannv = NullAgent::new("xuannv") as Arc<dyn Agent>;
        let xuannv_id = xuannv.card().id;
        shelf
            .insert(ShelfEntry {
                card: xuannv.card().clone(),
                agent: xuannv,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(3600)),
            })
            .await;

        let bus = EventBus::with_memory_store().await.unwrap();
        let mut sub = bus.subscribe();
        let shutdowner = CountingShutdowner::new();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner.clone(),
            bus.clone(),
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
        .with_xuannv_exempt({
            let (_tx, rx) = tokio::sync::watch::channel(Some(xuannv_id));
            // _tx 必须 leak 否则 rx 立即 closed；测试里直接 forget。
            std::mem::forget(_tx);
            rx
        });
        let n = gc.tick_once().await.unwrap();
        assert_eq!(n, 0, "xuannv 不应进入 shutdown 计数");
        assert_eq!(
            shutdowner.calls.load(Ordering::Relaxed),
            0,
            "shutdowner 不该被调用"
        );

        // bus 上不应有 AgentShuttingDown for xuannv
        let mut saw_shutdown = false;
        for _ in 0..3 {
            if let Ok(Some(Ok(ev))) =
                tokio::time::timeout(Duration::from_millis(50), sub.next()).await
                && let EventKind::AgentShuttingDown { .. } = &ev.kind
                && ev.meta.agent == Some(xuannv_id)
            {
                saw_shutdown = true;
                break;
            }
        }
        assert!(
            !saw_shutdown,
            "bus 上不应出现针对 xuannv 的 AgentShuttingDown"
        );
    }

    /// Bug 修：xuannv 豁免不影响其他 idle 门客被正常 GC。
    #[tokio::test]
    async fn idle_gc_xuannv_exempt_does_not_block_others() {
        let shelf = Arc::new(Shelf::new());

        let xuannv = NullAgent::new("xuannv") as Arc<dyn Agent>;
        let xuannv_id = xuannv.card().id;
        shelf
            .insert(ShelfEntry {
                card: xuannv.card().clone(),
                agent: xuannv,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(3600)),
            })
            .await;

        let worker = NullAgent::new("luban") as Arc<dyn Agent>;
        let worker_id = worker.card().id;
        shelf
            .insert(ShelfEntry {
                card: worker.card().clone(),
                agent: worker,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(3600)),
            })
            .await;

        let bus = EventBus::with_memory_store().await.unwrap();
        let shutdowner = CountingShutdowner::new();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner.clone(),
            bus,
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
        .with_xuannv_exempt({
            let (_tx, rx) = tokio::sync::watch::channel(Some(xuannv_id));
            // _tx 必须 leak 否则 rx 立即 closed；测试里直接 forget。
            std::mem::forget(_tx);
            rx
        });
        let n = gc.tick_once().await.unwrap();
        assert_eq!(n, 1, "应只回收 1 只（worker），跳过 xuannv");
        let hits = shutdowner.hits.lock().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, worker_id);
    }

    /// 块2.3：池模式下，idle 超 TTL 的分身走 dormant 路径——池中映射被移除，
    /// 且仍发出 `shutdown_idle`（进程回收），但**不**走永久 kill 旁路。
    /// 这是单玄女 skip 语义的升级：分身可被回收 + 后续 respawn。
    #[tokio::test]
    async fn idle_gc_reaps_dormant_not_permanent_kill_of_xuannv_clone() {
        use crate::xuannv_pool::XuannvPool;
        use fuxi_core::TopicId;

        let shelf = Arc::new(Shelf::new());
        let clone = NullAgent::new("xuannv") as Arc<dyn Agent>;
        let clone_id = clone.card().id;
        shelf
            .insert(ShelfEntry {
                card: clone.card().clone(),
                agent: clone,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(3600)),
            })
            .await;

        let pool = Arc::new(XuannvPool::new(3));
        let topic = TopicId::new();
        pool.set_active(topic, clone_id).await;

        let bus = EventBus::with_memory_store().await.unwrap();
        let shutdowner = CountingShutdowner::new();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner.clone(),
            bus,
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
        .with_xuannv_pool(pool.clone());

        let n = gc.tick_once().await.unwrap();
        assert_eq!(n, 1, "分身应被 dormant 回收（计入 shutdown）");
        // 池中映射已移除——下次 active_id 为 None（dormant），可被 respawn 重建。
        assert_eq!(
            pool.active_id(topic).await,
            None,
            "dormant 后池中 topic 映射应被移除"
        );
        // shutdown_idle 被调，reason=idle_ttl（非永久 kill 旁路）。
        let hits = shutdowner.hits.lock().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, clone_id);
        assert_eq!(hits[0].1, "idle_ttl");
    }

    /// 块5 修（2026-06-10 home 实测撞穿）：**general 分身是常驻 home base，
    /// 永不 dormant 回收**。根因——general 被回收后 `xuannv_id` 置 None，而
    /// `ensure_xuannv` 只在 boot 调一次、没有任何 HTTP 请求触发它 respawn →
    /// general 玄女永久 503 到重启。复现：home general 04:25 被 GC，9.5h 后用户
    /// 发消息一律「玄女尚未就绪」。治法同 CLAUDE.md 公理「IdleGcTask 必须豁免玄女」，
    /// 池模式下豁免对象 = general topic 分身（非 general 的 topic 分身仍可 dormant，
    /// 由 bridge 里程碑 respawn）。
    #[tokio::test]
    async fn idle_gc_never_reclaims_general_clone() {
        use crate::xuannv_pool::XuannvPool;
        use fuxi_core::TopicId;

        let shelf = Arc::new(Shelf::new());
        let clone = NullAgent::new("xuannv") as Arc<dyn Agent>;
        let clone_id = clone.card().id;
        shelf
            .insert(ShelfEntry {
                card: clone.card().clone(),
                agent: clone,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(3600)),
            })
            .await;

        let pool = Arc::new(XuannvPool::new(3));
        // general topic 的分身——绝不能被 GC 回收。
        pool.set_active(TopicId::general(), clone_id).await;

        let bus = EventBus::with_memory_store().await.unwrap();
        let shutdowner = CountingShutdowner::new();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner.clone(),
            bus,
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
        .with_xuannv_pool(pool.clone());

        let n = gc.tick_once().await.unwrap();
        assert_eq!(n, 0, "general 分身不该被回收（计 0）");
        assert_eq!(
            pool.active_id(TopicId::general()).await,
            Some(clone_id),
            "general 分身映射必须保留，否则 xuannv_id 置 None 后无 respawn 入口 → 永久 503"
        );
        assert!(
            shutdowner.hits.lock().unwrap().is_empty(),
            "general 分身不该触发任何 shutdown_idle"
        );
    }

    #[tokio::test]
    async fn idle_gc_respects_configured_ttl() {
        // TTL=1h，门客只 idle 5 分钟 → 不该被回收
        let shelf = Arc::new(Shelf::new());
        let agent = NullAgent::new("dev") as Arc<dyn Agent>;
        shelf
            .insert(ShelfEntry {
                card: agent.card().clone(),
                agent,
                status: ShelfStatus::Idle,
                worktree: None,
                idle_since: Some(Instant::now() - Duration::from_secs(300)),
            })
            .await;

        let shutdowner = CountingShutdowner::new();
        let bus = EventBus::with_memory_store().await.unwrap();
        let gc = IdleGcTask::new(
            shelf.clone(),
            shutdowner.clone(),
            bus,
            Duration::from_secs(3600),
            Duration::from_secs(30),
        );
        let n = gc.tick_once().await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(shutdowner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ttl_from_env_defaults_and_parses() {
        // 默认（env 未设）
        // SAFETY: test-only env 操作，串行模块内无并发冲突
        unsafe {
            std::env::remove_var("FUXI_IDLE_TTL_SECS");
        }
        assert_eq!(ttl_from_env(), Duration::from_secs(DEFAULT_IDLE_TTL_SECS));

        // 自定义
        unsafe {
            std::env::set_var("FUXI_IDLE_TTL_SECS", "42");
        }
        assert_eq!(ttl_from_env(), Duration::from_secs(42));

        // 非法 → 回落 default
        unsafe {
            std::env::set_var("FUXI_IDLE_TTL_SECS", "not-a-number");
        }
        assert_eq!(ttl_from_env(), Duration::from_secs(DEFAULT_IDLE_TTL_SECS));

        unsafe {
            std::env::remove_var("FUXI_IDLE_TTL_SECS");
        }
    }
}
