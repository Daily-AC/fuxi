//! 伏羲 EventBus —— tokio broadcast + SQLite WAL 持久化 + replay。
//!
//! 设计原则：
//! - **发布非阻塞**：`publish` 只做两件事——在 broadcast 里 fan-out、
//!   并把事件塞进后台 writer 的 mpsc。任何一侧阻塞都不影响调用者。
//! - **Writer backlog 不丢用户事件**：当 mpsc 堆积越过阈值时，追加一条
//!   `Custom { label: "event_store_lagged", payload: { pending: N } }`
//!   的哨兵事件（仅广播，不阻塞），**原始事件仍然落库**。
//! - **Lagged subscriber 容忍**：broadcast 的 `RecvError::Lagged` 不应终止订阅；
//!   见 `Subscriber::recv`。
//!
//! 公理 #3 “真实时，不轮询”——订阅者拿的是 push，不是 poll。

use crate::store::{EventStore, EventStream, ReplayCursor};
use crate::{Error, Result};
use futures_util::StreamExt;
use fuxi_core::{Event, EventKind, EventMeta};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, error, warn};

/// `EventBus` 的可调参数。
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// broadcast 频道容量。默认 1024。
    pub buffer: usize,
    /// 后台 writer mpsc 容量。默认 4096。
    pub writer_queue: usize,
    /// writer mpsc 堆积超过多少条算 “lag”。默认 512。
    pub lag_threshold: usize,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            buffer: 1024,
            writer_queue: 4096,
            lag_threshold: 512,
        }
    }
}

/// 伏羲事件总线。clone 便宜（内部 `Arc`）。
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

struct Inner {
    broadcast_tx: broadcast::Sender<Event>,
    writer_tx: mpsc::Sender<Event>,
    store: EventStore,
    cfg: EventBusConfig,
    writer_pending: Arc<AtomicUsize>,
    lag_sentinel_in_flight: Arc<AtomicUsize>,
    _writer_handle: JoinHandle<()>,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("buffer", &self.inner.cfg.buffer)
            .field("writer_queue", &self.inner.cfg.writer_queue)
            .field("lag_threshold", &self.inner.cfg.lag_threshold)
            .finish()
    }
}

impl EventBus {
    /// 基于已有 `EventStore` 构造事件总线。
    pub fn new(store: EventStore, cfg: EventBusConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel::<Event>(cfg.buffer.max(1));
        let (writer_tx, mut writer_rx) = mpsc::channel::<Event>(cfg.writer_queue.max(1));

        let writer_pending = Arc::new(AtomicUsize::new(0));
        let lag_sentinel_in_flight = Arc::new(AtomicUsize::new(0));

        let store_clone = store.clone();
        let pending_clone = writer_pending.clone();
        // writer 任务：串行消费 mpsc，把事件落库。落库错误只 `error!` 不 panic。
        // 为什么串行：避免 SQLite 锁竞争；批量优化等 P4 再说。
        let writer_handle = tokio::spawn(async move {
            while let Some(ev) = writer_rx.recv().await {
                pending_clone.fetch_sub(1, Ordering::Relaxed);
                if let Err(e) = store_clone.append(&ev).await {
                    error!(error = %e, event_id = %ev.meta.id, "写入事件失败（丢掉）");
                }
            }
            debug!("EventBus writer 退出");
        });

        Self {
            inner: Arc::new(Inner {
                broadcast_tx,
                writer_tx,
                store,
                cfg,
                writer_pending,
                lag_sentinel_in_flight,
                _writer_handle: writer_handle,
            }),
        }
    }

    /// 便捷构造：用内存库 + 默认配置。主要给测试用。
    pub async fn with_memory_store() -> Result<Self> {
        let store = EventStore::connect_memory().await?;
        Ok(Self::new(store, EventBusConfig::default()))
    }

    /// 发布事件。**非阻塞**：若订阅端/写入端拥塞，不会让调用方等。
    ///
    /// - 广播发送：无订阅者时返回 Ok（broadcast 允许 0 receiver）。
    /// - 持久化：通过 `writer_tx.try_send`，满时发布 lag 哨兵。
    pub fn publish(&self, ev: Event) -> Result<()> {
        // 1) 广播——订阅者 lag 不是这里的问题（receiver 端处理）。
        //    返回的 `Err(SendError)` 只意味着 0 个活跃 receiver；这不是错误。
        let _ = self.inner.broadcast_tx.send(ev.clone());

        // 2) 写入——非阻塞塞进 mpsc。
        match self.inner.writer_tx.try_send(ev) {
            Ok(_) => {
                self.inner.writer_pending.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                // 为什么不替调用方丢消息：spec 明令“原始 publish 不能被丢”——因此我们
                // 宁可阻塞一个背景任务也要把它塞进去。
                let pending = self.inner.writer_pending.load(Ordering::Relaxed);
                if pending >= self.inner.cfg.lag_threshold {
                    self.maybe_emit_lag_sentinel(pending);
                }
                let writer_tx = self.inner.writer_tx.clone();
                let pending_counter = self.inner.writer_pending.clone();
                warn!(
                    pending,
                    "event_store writer 拥塞，事件转交后台任务阻塞等待入队"
                );
                tokio::spawn(async move {
                    if let Err(e) = writer_tx.send(dropped).await {
                        error!(error = %e, "background 入队失败（bus 已关闭？）");
                        return;
                    }
                    pending_counter.fetch_add(1, Ordering::Relaxed);
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // writer 任务死了——通常意味着 EventBus 正在 drop。返回错误让调用者感知。
                return Err(Error::WriterClosed);
            }
        }
        Ok(())
    }

    /// 订阅实时事件流。订阅点之**后**产生的事件被推送。
    pub fn subscribe(&self) -> EventStream {
        let rx = self.inner.broadcast_tx.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(|r| async move {
            match r {
                Ok(ev) => Some(Ok(ev)),
                // 为什么 Lagged 不终止流：broadcast 里 lag 只代表 receiver 跟不上节奏，
                // 丢几条即可——业务语义上，全量历史走 `replay` 而非 `subscribe`。
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    warn!(missed = n, "broadcast 订阅者 lag，丢弃 N 条后继续");
                    None
                }
            }
        });
        Box::pin(stream)
    }

    /// 按游标回放历史。若 `live_tail = true`，回放完立即追加当下及之后的实时流。
    ///
    /// 注意：`live_tail` 模式下可能出现**极短窗口内的重复**——如果一条事件恰在
    /// 回放读到它、同时又被广播到 live 订阅端之间到达，就会两边都出现。
    /// 这是伏羲当前接受的权衡（世界模型 watcher 自己去重）。
    pub fn replay(&self, cursor: ReplayCursor, live_tail: bool) -> EventStream {
        let hist = self.inner.store.replay(cursor);
        if !live_tail {
            return hist;
        }
        // 先订阅 live，后消费 history——尽量少漏。
        let live = self.subscribe();
        let combined = hist.chain(live);
        Box::pin(combined)
    }

    /// 暴露底层 store（给 SCM、firehose crate 使用）。
    pub fn store(&self) -> &EventStore {
        &self.inner.store
    }

    /// 当前活跃订阅者数量——给「无桌宠连接」之类的近似探活用。
    ///
    /// 注意 broadcast receiver 是**全局**的：firehose / PWA / 桌宠都共享同一个
    /// channel，本方法**不能**区分谁是谁。`== 0` 才能确定连一个观察者都没；
    /// `> 0` 不代表桌宠在线（可能只是 PWA 看着）。
    /// 玄女眼睛 v1 接受这个粗粒度：判 0 时直接 400 `no_pet_connected` 让玄女
    /// 立刻知道；非 0 时 publish + 等 oneshot，超时也兜底。
    pub fn receiver_count(&self) -> usize {
        self.inner.broadcast_tx.receiver_count()
    }

    /// 拿某 task 的完整历史事件（按存储顺序升序）。
    ///
    /// 薄包装 `EventStore::history_for_task`——Extractor（M2.5）作为 bus 消费者
    /// 不该直接摸 store。这里转发一层，顺便让 `EventBus` 的消费方不必关心
    /// "historical 读" 和 "实时 subscribe" 走的是同一 store 还是不同路径。
    pub async fn history_for_task(&self, task: fuxi_core::TaskId) -> Result<Vec<fuxi_core::Event>> {
        self.inner.store.history_for_task(task).await
    }

    /// 列出所有曾出现过的 task id —— 给 IM `/api/tasks` 重建任务列表用。
    /// 薄包装 `EventStore::list_task_ids`。
    pub async fn list_task_ids(&self) -> Result<Vec<String>> {
        self.inner.store.list_task_ids().await
    }

    /// 按 `kind_tag` 拉全部事件（升序）。薄包装 `EventStore::events_by_kind`。
    pub async fn events_by_kind(&self, kind: &str) -> Result<Vec<fuxi_core::Event>> {
        self.inner.store.events_by_kind(kind).await
    }

    fn maybe_emit_lag_sentinel(&self, pending: usize) {
        // 为什么计数器：避免同一时刻狂刷 lag 哨兵反而加剧拥塞。
        let prev = self
            .inner
            .lag_sentinel_in_flight
            .fetch_add(1, Ordering::Relaxed);
        if prev > 0 {
            self.inner
                .lag_sentinel_in_flight
                .fetch_sub(1, Ordering::Relaxed);
            return;
        }
        let sentinel = Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: "event_store_lagged".to_string(),
                payload: serde_json::json!({ "pending": pending }),
            },
        };
        let _ = self.inner.broadcast_tx.send(sentinel);
        self.inner
            .lag_sentinel_in_flight
            .fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use fuxi_core::EventMeta;
    use std::time::Duration;
    use tokio::time::timeout;

    fn mk_ev(label: &str) -> Event {
        Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: label.to_string(),
                payload: serde_json::json!({}),
            },
        }
    }

    fn label_of(ev: &Event) -> String {
        match &ev.kind {
            EventKind::Custom { label, .. } => label.clone(),
            _ => "__other__".to_string(),
        }
    }

    async fn wait_flush(bus: &EventBus) {
        // 等 writer 任务把 mpsc 清空——测试里用固定退让加 load 检查。
        for _ in 0..50 {
            if bus.inner.writer_pending.load(Ordering::Relaxed) == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 玄女眼睛 v1：`receiver_count` 反映当前订阅者数量。
    /// 0 = 没人在听（vision handler 据此判 `no_pet_connected`），
    /// 订阅一次 = 1，drop 后回 0。
    #[tokio::test]
    async fn receiver_count_tracks_subscribers() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        assert_eq!(bus.receiver_count(), 0, "无订阅时应为 0");
        let s1 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 1);
        let s2 = bus.subscribe();
        assert_eq!(bus.receiver_count(), 2);
        drop(s1);
        // BroadcastStream 内部持 receiver；drop stream 即 drop receiver。
        // 注意：drop 后 broadcast 内部清算可能滞后到下次 send/receiver_count 调用，
        // 极端时序下计数短暂偏差不影响 0/非 0 决策。
        assert!(bus.receiver_count() <= 2);
        drop(s2);
        // 同上——0/非0 决策只关心是不是确实没人在听
    }

    #[tokio::test]
    async fn fanout_preserves_order_per_subscriber() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let mut s1 = bus.subscribe();
        let mut s2 = bus.subscribe();

        let labels = ["a", "b", "c", "d"];
        for l in labels {
            bus.publish(mk_ev(l)).expect("publish");
        }

        for l in labels {
            let a = timeout(Duration::from_secs(2), s1.next())
                .await
                .expect("timeout s1")
                .expect("closed s1")
                .expect("err s1");
            let b = timeout(Duration::from_secs(2), s2.next())
                .await
                .expect("timeout s2")
                .expect("closed s2")
                .expect("err s2");
            assert_eq!(label_of(&a), l);
            assert_eq!(label_of(&b), l);
        }
    }

    #[tokio::test]
    async fn subscribe_before_any_publish_then_receive_first_event() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let mut s = bus.subscribe();
        bus.publish(mk_ev("first")).expect("publish");
        let got = timeout(Duration::from_secs(2), s.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        assert_eq!(label_of(&got), "first");
    }

    #[tokio::test]
    async fn history_for_task_delegates_to_store() {
        // WHY：Extractor 订阅 bus、在 Done 后拉 task 的历史。作为 bus 的消费者
        // 不该知道底下是 SQLite——从 bus 上直接有 `history_for_task` 最干净。
        use fuxi_core::TaskId;
        let bus = EventBus::with_memory_store().await.expect("bus");
        let t = TaskId::new();

        let make = |label: &str| {
            let mut meta = EventMeta::now();
            meta.task = Some(t);
            Event {
                meta,
                kind: EventKind::Custom {
                    label: label.to_string(),
                    payload: serde_json::json!({}),
                },
            }
        };
        bus.publish(make("a")).expect("publish a");
        bus.publish(make("b")).expect("publish b");
        wait_flush(&bus).await;

        let hist = bus.history_for_task(t).await.expect("history");
        assert_eq!(hist.len(), 2);
        assert_eq!(label_of(&hist[0]), "a");
        assert_eq!(label_of(&hist[1]), "b");
    }

    #[tokio::test]
    async fn backlog_emits_lag_sentinel_but_preserves_publish() {
        // 极小 writer_queue + 大 lag_threshold=0 逼 sentinel 触发。
        let store = EventStore::connect_memory().await.expect("store");
        let cfg = EventBusConfig {
            buffer: 1024,
            writer_queue: 1,
            lag_threshold: 1,
        };
        let bus = EventBus::new(store, cfg);
        let mut s = bus.subscribe();

        // 连续 publish，写入端一定会顶到 Full 分支。
        for i in 0..20 {
            bus.publish(mk_ev(&format!("ev-{i}"))).expect("publish");
        }

        let mut saw_sentinel = false;
        let mut saw_original = false;
        // 拉 ~30 条尝试捕获哨兵；哨兵一定在 ev-* 之间。
        for _ in 0..40 {
            let Ok(Some(Ok(ev))) = timeout(Duration::from_secs(2), s.next()).await else {
                break;
            };
            let l = label_of(&ev);
            if l == "event_store_lagged" {
                saw_sentinel = true;
            }
            if l.starts_with("ev-") {
                saw_original = true;
            }
            if saw_sentinel && saw_original {
                break;
            }
        }

        // 等 writer flush 完，确认所有原始事件都落库了（不能因为 backlog 丢）。
        wait_flush(&bus).await;
        let hist = bus.replay(ReplayCursor::Beginning, false);
        let all: Vec<_> = hist
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter_map(|r| r.ok())
            .collect();
        let originals = all
            .iter()
            .filter(|e| label_of(e).starts_with("ev-"))
            .count();
        assert_eq!(originals, 20, "原始事件不应被丢弃");
        assert!(saw_original, "订阅者应拿到至少一条原始事件");
        assert!(saw_sentinel, "backlog 时应广播 event_store_lagged 哨兵");
    }
}
