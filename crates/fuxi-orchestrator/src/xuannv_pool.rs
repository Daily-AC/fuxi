//! 玄女分身池：topic_id → 活分身 AgentId。懒启动 + 空闲 dormant 回收 + LRU。
//!
//! 现状（block 2 之前）是单玄女 + 活跃 topic 切换，跨 topic 里程碑透传导致串味。
//! 本池把单玄女换成「按 topic 的分身映射」：每个 topic 一个活分身，超 `max_active`
//! 时按 LRU 回收最久未活跃的 topic（调用方负责 dormant 它的进程）。
//!
//! WHY watch 而非直接读 inner：bridge 等订阅者要实时跟随 respawn 后的 id 漂移
//! （见 memory `feedback_dynamic_agent_id_via_watch`——snapshot 一次的 id 在
//! handoff/respawn 后会失效）。`set_active` / `remove` 每次都 `send_replace` 全量
//! 快照，订阅者总拿到最新映射。
//!
//! WHY LRU 用单调计数而非时间戳：测试要确定性，且「最近活跃」只需相对顺序，
//! 不需要墙钟。每次 set/touch 把全局 `clock` bump 后记到该 topic。

use fuxi_core::{TopicId, id::AgentId};
use std::collections::HashMap;
use tokio::sync::{Mutex, watch};

/// 玄女分身池。
///
/// 映射只保留**活**分身——dormant 回收后从 `active` 移除（不留死 id，避免
/// 订阅者拿到已退出的进程 id）。
pub struct XuannvPool {
    /// 同时存活分身数上限。超出由 [`XuannvPool::lru_victim_if_over_cap`] 报告受害者。
    max_active: usize,
    inner: Mutex<PoolInner>,
    /// watch：供订阅者实时读 topic→分身全量映射（跟随 respawn 漂移）。
    tx: watch::Sender<HashMap<TopicId, AgentId>>,
}

struct PoolInner {
    active: HashMap<TopicId, AgentId>,
    /// LRU 近况：topic → 最近活跃单调计数（用计数避免墙钟依赖）。
    lru_tick: HashMap<TopicId, u64>,
    clock: u64,
}

impl XuannvPool {
    pub fn new(max_active: usize) -> Self {
        let (tx, _rx) = watch::channel(HashMap::new());
        Self {
            max_active,
            inner: Mutex::new(PoolInner {
                active: HashMap::new(),
                lru_tick: HashMap::new(),
                clock: 0,
            }),
            tx,
        }
    }

    /// 当前 topic 的活分身 id。无活分身（从未起 / 已 dormant）返回 None。
    /// 命中也顺手 touch LRU——「读到 = 在用」，避免活跃 topic 被误回收。
    pub async fn active_id(&self, topic: TopicId) -> Option<AgentId> {
        let mut inner = self.inner.lock().await;
        let id = inner.active.get(&topic).copied();
        if id.is_some() {
            inner.clock += 1;
            let c = inner.clock;
            inner.lru_tick.insert(topic, c);
        }
        id
    }

    /// 绑定 topic → 活分身（spawn / respawn 后调）。bump LRU 并广播新快照。
    pub async fn set_active(&self, topic: TopicId, id: AgentId) {
        let snapshot = {
            let mut inner = self.inner.lock().await;
            inner.active.insert(topic, id);
            inner.clock += 1;
            let c = inner.clock;
            inner.lru_tick.insert(topic, c);
            inner.active.clone()
        };
        self.tx.send_replace(snapshot);
    }

    /// dormant 回收：移除 topic 的活分身映射并广播。
    /// 不动 `lru_tick`——下次 set_active 会覆盖；保留旧 tick 不影响 victim 选择
    /// （victim 只看 active 内的 topic）。
    pub async fn remove(&self, topic: TopicId) {
        let snapshot = {
            let mut inner = self.inner.lock().await;
            inner.active.remove(&topic);
            inner.active.clone()
        };
        self.tx.send_replace(snapshot);
    }

    /// 订阅 topic→分身全量映射的实时视图。
    pub fn watch(&self) -> watch::Receiver<HashMap<TopicId, AgentId>> {
        self.tx.subscribe()
    }

    /// 反查：某 agent id 当前归属哪个 topic 的活分身。
    /// idle GC 只拿到 id，要 dormant 它必须先反查 topic 才能 [`XuannvPool::remove`]。
    /// 不命中（已被移除 / 非池中分身）返回 None。
    pub async fn topic_of(&self, id: AgentId) -> Option<TopicId> {
        let inner = self.inner.lock().await;
        inner
            .active
            .iter()
            .find_map(|(t, aid)| (*aid == id).then_some(*t))
    }

    /// 判定某 id 是否是池中任一活分身——shutdown_agent 豁免用：命中即拒绝永久 kill
    /// （玄女分身是用户对话入口，误 kill 等价于单玄女被杀的旧 bug）。
    pub async fn is_active_clone(&self, id: AgentId) -> bool {
        let inner = self.inner.lock().await;
        inner.active.values().any(|aid| *aid == id)
    }

    /// 超 `max_active` 时返回应被 LRU 回收的 (topic, 分身 id)（最久未活跃的活分身）。
    /// **general 永不入选**（永驻公理 9151f54——cap 对 general 等于少占一个名额）。
    /// 直接带回 agent id：调用方不要再 `active_id` 反查（那会 touch LRU 把 victim
    /// 救活）。调用方负责 dormant（进程回收 + [`XuannvPool::remove`]）。未超返回 None。
    pub async fn lru_victim_if_over_cap(&self) -> Option<(TopicId, AgentId)> {
        let inner = self.inner.lock().await;
        if inner.active.len() <= self.max_active {
            return None;
        }
        // 只在活分身里挑；lru_tick 缺失的 topic 视为最久（tick 0），优先回收。
        inner
            .active
            .iter()
            .filter(|(t, _)| **t != TopicId::general())
            .min_by_key(|(t, _)| inner.lru_tick.get(t).copied().unwrap_or(0))
            .map(|(t, a)| (*t, *a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_insert_and_lookup_by_topic() {
        let pool = XuannvPool::new(3); // max_active=3
        let a = AgentId::new();
        let topic = TopicId(uuid::Uuid::nil());
        pool.set_active(topic, a).await;
        assert_eq!(pool.active_id(topic).await, Some(a));
        assert_eq!(
            pool.active_id(TopicId(uuid::Uuid::from_u128(9))).await,
            None
        );
    }

    #[tokio::test]
    async fn pool_remove_drops_mapping() {
        let pool = XuannvPool::new(3);
        let a = AgentId::new();
        let topic = TopicId::new();
        pool.set_active(topic, a).await;
        pool.remove(topic).await;
        assert_eq!(pool.active_id(topic).await, None);
    }

    #[tokio::test]
    async fn pool_watch_reflects_latest_snapshot() {
        let pool = XuannvPool::new(3);
        let mut rx = pool.watch();
        let topic = TopicId::new();
        let a = AgentId::new();
        pool.set_active(topic, a).await;
        // borrow_and_update 拿最新快照
        let snap = rx.borrow_and_update().clone();
        assert_eq!(snap.get(&topic).copied(), Some(a));

        pool.remove(topic).await;
        let snap = rx.borrow_and_update().clone();
        assert!(!snap.contains_key(&topic));
    }

    #[tokio::test]
    async fn topic_of_reverse_lookup() {
        let pool = XuannvPool::new(3);
        let a = AgentId::new();
        let topic = TopicId(uuid::Uuid::from_u128(7));
        pool.set_active(topic, a).await;
        assert_eq!(pool.topic_of(a).await, Some(topic));
        // 不在池中的 id 反查不到。
        assert_eq!(pool.topic_of(AgentId::new()).await, None);
        // dormant 后反查不到。
        pool.remove(topic).await;
        assert_eq!(pool.topic_of(a).await, None);
    }

    #[tokio::test]
    async fn is_active_clone_tracks_membership() {
        let pool = XuannvPool::new(3);
        let a = AgentId::new();
        let topic = TopicId::new();
        assert!(!pool.is_active_clone(a).await);
        pool.set_active(topic, a).await;
        assert!(pool.is_active_clone(a).await);
        pool.remove(topic).await;
        assert!(!pool.is_active_clone(a).await);
    }

    #[tokio::test]
    async fn lru_victim_none_when_under_cap() {
        let pool = XuannvPool::new(3);
        for _ in 0..3 {
            pool.set_active(TopicId::new(), AgentId::new()).await;
        }
        // 恰好等于 cap，不该回收。
        assert_eq!(pool.lru_victim_if_over_cap().await.map(|(t, _)| t), None);
    }

    #[tokio::test]
    async fn lru_victim_picks_least_recently_active_over_cap() {
        let pool = XuannvPool::new(2);
        let t1 = TopicId(uuid::Uuid::from_u128(1));
        let t2 = TopicId(uuid::Uuid::from_u128(2));
        let t3 = TopicId(uuid::Uuid::from_u128(3));

        pool.set_active(t1, AgentId::new()).await; // tick 1
        pool.set_active(t2, AgentId::new()).await; // tick 2
        // t1 被重新触达 → 比 t2 更近。
        let _ = pool.active_id(t1).await; // tick 3
        pool.set_active(t3, AgentId::new()).await; // tick 4 —— 现在 3 个，超 cap=2

        // t2 是最久未活跃的，应被选为受害者。
        assert_eq!(
            pool.lru_victim_if_over_cap().await.map(|(t, _)| t),
            Some(t2)
        );
    }

    #[tokio::test]
    async fn lru_victim_never_selects_general() {
        // general 永驻公理（9151f54）：即使 general 是最久未活跃，victim 也必须
        // 跳过它选下一个。
        let pool = XuannvPool::new(1);
        let general = TopicId::general();
        let t2 = TopicId(uuid::Uuid::from_u128(2));
        pool.set_active(general, AgentId::new()).await; // tick 1（最老）
        pool.set_active(t2, AgentId::new()).await; // tick 2 —— 超 cap=1

        let victim = pool.lru_victim_if_over_cap().await;
        assert_eq!(
            victim.map(|(t, _)| t),
            Some(t2),
            "general 豁免，victim 应是 t2"
        );
    }

    #[tokio::test]
    async fn lru_victim_returns_agent_id_without_touching_lru() {
        // victim 返 (topic, agent)——调用方拿 id 直接 shutdown，不再 active_id
        // 反查（active_id 会 touch LRU，把 victim 救活成最新）。
        let pool = XuannvPool::new(0);
        let t = TopicId(uuid::Uuid::from_u128(7));
        let a = AgentId::new();
        pool.set_active(t, a).await;
        assert_eq!(pool.lru_victim_if_over_cap().await, Some((t, a)));
        // 连问两次仍是它——证明没 touch LRU
        assert_eq!(pool.lru_victim_if_over_cap().await, Some((t, a)));
    }
}
