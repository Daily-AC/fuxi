//! worker → controller `/dist/event` 客户端。
//!
//! ## 角色定位
//!
//! 这是 P2 跨节点 EventBus 的 worker 侧出口：本地子门客（cc/codex 等）的
//! `Event` 经由 `NetworkBusClient::enqueue` 攒一小批后 POST 给 controller，
//! controller 端 republish 到中心 bus，TUI/玄女据此能"看见"远端 worker 上的
//! 子 agent 实时输出。
//!
//! ## 与 cc `pending.rs` 的对照
//!
//! 两边都是有界队列 + drop oldest，但语义不同：
//! - cc pending：busy 期间 enqueue 用户输入，turn terminal 一次性 drain；
//!   单消费者（pump）。
//! - NetworkBusClient：连续 enqueue 事件，独立 `flush_loop` 后台 task 按时间或
//!   批量上限 trigger HTTP send；一对多（agent 调 enqueue → loop 攒批 → POST）。
//!
//! ## 为什么独立成 module
//!
//! `dist.rs` 已 ~3850 行；新文件避免再涨 + 纯 IO 层逻辑可单测，不必 spawn
//! 真 controller（用 mock endpoint 即可）。
//!
//! 本模块是 P2 任务 #2 的纯 IO 层；调用方（γ：worker 子门客 active_tx 桥接）
//! 由后续任务接入。在 γ 接入前，构造函数 / 默认常量未被生产路径调用——
//! 模块级 `#[allow(dead_code)]` 让 `cargo clippy -D warnings` 通过。

#![allow(dead_code)]

use crate::dist::{DistEventReq, DistEventResp};
use fuxi_core::event::Event;
use reqwest::Client;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

/// 队列默认上限——按 ~4 events/s × 60s 估，覆盖一分钟级 controller 不可达。
/// 再大没意义：长时间断网应让上层重连而不是堆内存。
pub const DEFAULT_QUEUE_CAP: usize = 256;

/// 单批触发上限——攒到这么多就立即 flush 不等 tick；保证爆发时延迟可控。
pub const DEFAULT_BATCH_SIZE: usize = 16;

/// flush_loop tick 周期；与 cc 默认 progress flush 对齐。
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_millis(200);

/// retry 退避序列——4 次后放弃整 batch。`[200, 500, 1000, 2000]` 累计 ≈ 3.7s，
/// controller 暂时 503 / 网络抖动覆盖足够；超出说明 controller 真死，重试无用。
pub const DEFAULT_RETRY_BACKOFF_MS: &[u64] = &[200, 500, 1000, 2000];

/// 入队结果——上层据此可埋日志/指标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Queued,
    /// 队列满，最旧一条被驱逐（其内容已丢，仅记数即可）。
    DroppedOldest,
}

/// worker → controller 的事件转发客户端。`Arc<Self>` 共享：
/// - 多个 agent pump 调 `enqueue`
/// - 一个后台 `flush_loop` 消费
/// - `shutdown` 触发最后一次 flush 并停 loop
pub struct NetworkBusClient {
    inner: Arc<Mutex<Inner>>,
    queue_cap: usize,
    batch_size: usize,
    flush_interval: Duration,
    retry_backoff_ms: Vec<u64>,
    /// HTTP 配置——`flush_loop` 用它构 POST。
    transport: Transport,
    /// flush_loop 看到 `notify_one()` 立即跑一次，不必等 tick；用于 batch_size
    /// 满 + shutdown 触发立即 flush。
    flush_signal: Arc<Notify>,
}

struct Inner {
    queue: VecDeque<Event>,
    /// shutdown 信号——flush_loop 看到 true 就 drain 后退出。
    shutdown: bool,
    /// 累计 dropped oldest 计数；测试断言 + 未来 metrics 复用。
    dropped_count: u64,
}

#[derive(Clone)]
struct Transport {
    client: Client,
    controller: String,
    token: String,
    node_id: String,
}

impl NetworkBusClient {
    /// 用 worker 上下文构造——`flush_loop` 启动前调用。
    pub fn new(client: Client, controller: String, token: String, node_id: String) -> Self {
        Self::with_config(
            client,
            controller,
            token,
            node_id,
            DEFAULT_QUEUE_CAP,
            DEFAULT_BATCH_SIZE,
            DEFAULT_FLUSH_INTERVAL,
            DEFAULT_RETRY_BACKOFF_MS.to_vec(),
        )
    }

    /// 全参数构造——主要给单测控制 backoff/cap 缩短跑测时间。
    #[allow(clippy::too_many_arguments)]
    pub fn with_config(
        client: Client,
        controller: String,
        token: String,
        node_id: String,
        queue_cap: usize,
        batch_size: usize,
        flush_interval: Duration,
        retry_backoff_ms: Vec<u64>,
    ) -> Self {
        debug_assert!(queue_cap > 0, "queue_cap must be > 0");
        debug_assert!(batch_size > 0, "batch_size must be > 0");
        Self {
            inner: Arc::new(Mutex::new(Inner {
                queue: VecDeque::with_capacity(queue_cap.min(1024)),
                shutdown: false,
                dropped_count: 0,
            })),
            queue_cap,
            batch_size,
            flush_interval,
            retry_backoff_ms,
            transport: Transport {
                client,
                controller,
                token,
                node_id,
            },
            flush_signal: Arc::new(Notify::new()),
        }
    }

    /// push 一条 event 进 queue；满时驱逐队首并计数。
    /// queue 长度 >= batch_size 时 notify flush_loop 立即跑一轮。
    pub async fn enqueue(&self, event: Event) -> EnqueueOutcome {
        let outcome;
        let len_after;
        {
            let mut g = self.inner.lock().await;
            if g.queue.len() >= self.queue_cap {
                g.queue.pop_front();
                g.queue.push_back(event);
                g.dropped_count += 1;
                tracing::warn!(
                    queue_cap = self.queue_cap,
                    dropped_total = g.dropped_count,
                    "NetworkBusClient queue overflow, dropped oldest event"
                );
                outcome = EnqueueOutcome::DroppedOldest;
            } else {
                g.queue.push_back(event);
                outcome = EnqueueOutcome::Queued;
            }
            len_after = g.queue.len();
        }
        if len_after >= self.batch_size {
            self.flush_signal.notify_one();
        }
        outcome
    }

    /// 当前队列长度——主要给测试 + 调试用。
    pub async fn queue_len(&self) -> usize {
        self.inner.lock().await.queue.len()
    }

    /// 累计 drop oldest 次数。
    pub async fn dropped_count(&self) -> u64 {
        self.inner.lock().await.dropped_count
    }

    /// 标记 shutdown——flush_loop 看到后 drain 一次再退出。生产路径走 [`shutdown`]。
    async fn mark_shutdown(&self) {
        self.inner.lock().await.shutdown = true;
        self.flush_signal.notify_one();
    }

    /// 是否已 shutdown。
    pub async fn is_shutdown(&self) -> bool {
        self.inner.lock().await.shutdown
    }

    /// 取出最多 `n` 条事件——flush_loop 一个 batch 调用。空 vec 表示无事件。
    async fn take_batch(&self, n: usize) -> Vec<Event> {
        let mut g = self.inner.lock().await;
        let take = g.queue.len().min(n);
        g.queue.drain(..take).collect()
    }

    /// retry 失败时把 batch 重新前置塞回队首——保事件顺序，不让新 enqueue
    /// 的事件插队到旧事件前。当前 flush 路径不走 requeue（持 owned batch 重试），
    /// 此方法保留给未来"shutdown 期间最后一次失败要 surface 给上层"的扩展。
    #[allow(dead_code)]
    async fn requeue_front(&self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        let mut g = self.inner.lock().await;
        for ev in events.into_iter().rev() {
            if g.queue.len() >= self.queue_cap {
                g.queue.pop_back();
                g.dropped_count += 1;
            }
            g.queue.push_front(ev);
        }
    }

    /// spawn 后台 flush task。返回 JoinHandle 让上层在退出路径 await/abort。
    /// 触发条件：tick 周期到 / batch_size 达成 notify / shutdown notify。
    pub fn spawn_flush_loop(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.flush_interval);
            // 第一个 tick 立即 ready——跳过避免起步时多余空 POST。
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = self.flush_signal.notified() => {}
                }
                let stopping = self.is_shutdown().await;
                self.flush_once().await;
                if stopping {
                    // shutdown 后再排空一次——notify 与 enqueue 之间可能仍有
                    // batch_size 未达的尾巴；多排一次保不漏。
                    self.flush_once().await;
                    break;
                }
            }
        })
    }

    /// 调用方语义触发 shutdown 并等 flush task 跑完最后一轮。
    pub async fn shutdown(self: &Arc<Self>, handle: JoinHandle<()>) {
        self.mark_shutdown().await;
        let _ = handle.await;
    }

    /// 取一个 batch 并 POST；失败时 retry 直到 backoff 用完，最终失败 drop。
    /// **不持锁跨 await**——drain 后立即释放，POST 期间 enqueue 不阻塞。
    async fn flush_once(&self) {
        let batch = self.take_batch(self.batch_size).await;
        if batch.is_empty() {
            return;
        }
        let batch_len = batch.len();
        let mut attempt: usize = 0;
        loop {
            match self.post_once(&batch).await {
                Ok(()) => return,
                Err(err) => {
                    if attempt >= self.retry_backoff_ms.len() {
                        tracing::warn!(
                            batch_len,
                            attempts = attempt + 1,
                            error = %err,
                            "NetworkBusClient drop batch after max retries"
                        );
                        return;
                    }
                    let wait = Duration::from_millis(self.retry_backoff_ms[attempt]);
                    attempt += 1;
                    tracing::debug!(
                        batch_len,
                        attempt,
                        backoff_ms = wait.as_millis() as u64,
                        error = %err,
                        "NetworkBusClient retrying batch"
                    );
                    tokio::time::sleep(wait).await;
                    // batch 在 attempt 间不变——不 requeue，避免触发 cap 驱逐打乱顺序
                }
            }
        }
    }

    /// 单次 POST。返回 Err 触发 retry。
    async fn post_once(&self, events: &[Event]) -> Result<(), String> {
        let req = DistEventReq {
            token: self.transport.token.clone(),
            node_id: self.transport.node_id.clone(),
            events: events.to_vec(),
        };
        let resp = self
            .transport
            .client
            .post(format!("{}/dist/event", self.transport.controller))
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("send: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("status {status}"));
        }
        // accepted 字段 v1 我们假设 == events.len()；后续 partial accept 再处理。
        let _ack: DistEventResp = resp.json().await.map_err(|e| format!("decode resp: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::{DistController, router};
    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use fuxi_core::event::{EventKind, EventMeta};
    use fuxi_events::EventBus;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_event(role: &str) -> Event {
        Event {
            meta: EventMeta::now(),
            kind: EventKind::AgentSpawning {
                role: role.to_string(),
                cli: "codex".to_string(),
            },
        }
    }

    fn dummy_client(queue_cap: usize, batch_size: usize) -> NetworkBusClient {
        NetworkBusClient::with_config(
            Client::new(),
            "http://127.0.0.1:1".into(),
            "tok".into(),
            "node".into(),
            queue_cap,
            batch_size,
            Duration::from_millis(200),
            vec![],
        )
    }

    /// TDD #1：满时驱逐最旧一条；新 event 留在队尾。
    #[tokio::test]
    async fn enqueue_overflow_drops_oldest() {
        let client = dummy_client(3, 16);
        assert_eq!(
            client.enqueue(make_event("a")).await,
            EnqueueOutcome::Queued
        );
        assert_eq!(
            client.enqueue(make_event("b")).await,
            EnqueueOutcome::Queued
        );
        assert_eq!(
            client.enqueue(make_event("c")).await,
            EnqueueOutcome::Queued
        );
        assert_eq!(
            client.enqueue(make_event("d")).await,
            EnqueueOutcome::DroppedOldest
        );
        assert_eq!(client.queue_len().await, 3);
        assert_eq!(client.dropped_count().await, 1);

        let batch = client.take_batch(10).await;
        let roles: Vec<_> = batch
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::AgentSpawning { role, .. } => Some(role.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(roles, vec!["b", "c", "d"]);
    }

    /// TDD #2：mark_shutdown 后 is_shutdown=true，剩余队列仍可被 drain。
    #[tokio::test]
    async fn shutdown_flag_does_not_drop_pending() {
        let client = dummy_client(8, 16);
        client.enqueue(make_event("x")).await;
        client.enqueue(make_event("y")).await;
        client.mark_shutdown().await;
        assert!(client.is_shutdown().await);
        assert_eq!(client.queue_len().await, 2, "shutdown 不该清掉 pending");
        let batch = client.take_batch(8).await;
        assert_eq!(batch.len(), 2);
    }

    /// TDD #3：take_batch 上限 = min(queue_len, n)；超额请求只取现有量。
    #[tokio::test]
    async fn take_batch_respects_n_and_drains() {
        let client = dummy_client(10, 16);
        for i in 0..5 {
            client.enqueue(make_event(&format!("e{i}"))).await;
        }
        let first = client.take_batch(2).await;
        assert_eq!(first.len(), 2);
        assert_eq!(client.queue_len().await, 3);
        let rest = client.take_batch(100).await;
        assert_eq!(rest.len(), 3);
        assert_eq!(client.queue_len().await, 0);
    }

    /// TDD #4：requeue_front 失败 retry 路径——保 batch 内部顺序 + 前置到队首。
    #[tokio::test]
    async fn requeue_front_preserves_order_and_prepends() {
        let client = dummy_client(8, 16);
        client.enqueue(make_event("new1")).await;
        client.enqueue(make_event("new2")).await;
        let failed = vec![make_event("old1"), make_event("old2")];
        client.requeue_front(failed).await;
        let drained = client.take_batch(10).await;
        let roles: Vec<_> = drained
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::AgentSpawning { role, .. } => Some(role.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(roles, vec!["old1", "old2", "new1", "new2"]);
    }

    /// TDD #5：requeue_front 撞 cap 时丢的是新（队尾）那批——旧的优先保留。
    #[tokio::test]
    async fn requeue_front_overflow_drops_newest() {
        let client = dummy_client(3, 16);
        client.enqueue(make_event("n1")).await;
        client.enqueue(make_event("n2")).await;
        client.enqueue(make_event("n3")).await;
        client
            .requeue_front(vec![make_event("o1"), make_event("o2")])
            .await;
        assert_eq!(client.queue_len().await, 3);
        assert_eq!(client.dropped_count().await, 2);
        let drained = client.take_batch(10).await;
        let roles: Vec<_> = drained
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::AgentSpawning { role, .. } => Some(role.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(roles, vec!["o1", "o2", "n1"]);
    }

    // ── 集成测：mock controller endpoint ────────────────────────────

    /// 起一个真 axum controller（沿用 dist::router）让 client 真发 HTTP。
    async fn spawn_real_controller() -> (Arc<DistController>, String, tokio::task::JoinHandle<()>) {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ctrl = Arc::new(DistController::new("tok".into(), bus));
        ctrl.register("nodeB".into(), vec![], 1).await;
        let app = router(ctrl.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        (ctrl, format!("http://{addr}"), handle)
    }

    /// fake controller，前 fail_count 次 503，之后 200——验证 retry 行为不依赖
    /// 真 controller 的 retry 状态机。
    #[derive(Clone)]
    struct FakeCtrl {
        fail_remaining: Arc<AtomicUsize>,
        success_count: Arc<AtomicUsize>,
        last_batch_len: Arc<AtomicUsize>,
    }

    async fn fake_handler(
        State(ctrl): State<FakeCtrl>,
        axum::Json(req): axum::Json<DistEventReq>,
    ) -> impl IntoResponse {
        let prev = ctrl.fail_remaining.load(Ordering::SeqCst);
        if prev > 0 {
            ctrl.fail_remaining.fetch_sub(1, Ordering::SeqCst);
            return (StatusCode::SERVICE_UNAVAILABLE, "fail").into_response();
        }
        ctrl.success_count.fetch_add(1, Ordering::SeqCst);
        ctrl.last_batch_len
            .store(req.events.len(), Ordering::SeqCst);
        axum::Json(DistEventResp {
            accepted: req.events.len(),
        })
        .into_response()
    }

    async fn spawn_fake_controller(
        fail_count: usize,
    ) -> (FakeCtrl, String, tokio::task::JoinHandle<()>) {
        let ctrl = FakeCtrl {
            fail_remaining: Arc::new(AtomicUsize::new(fail_count)),
            success_count: Arc::new(AtomicUsize::new(0)),
            last_batch_len: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/dist/event", post(fake_handler))
            .with_state(ctrl.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        (ctrl, format!("http://{addr}"), handle)
    }

    /// TDD #6：flush_once 把 batch POST 到真 controller 后，事件走 bus 能被订阅
    /// 到——端到端 publish 成功的最小切片。
    #[tokio::test]
    async fn flush_sends_batch_to_endpoint() {
        let (ctrl, base, srv) = spawn_real_controller().await;
        let bus = ctrl.bus().clone();

        let probe = tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut s = bus.subscribe();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            let mut got = 0;
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    return got;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && matches!(ev.kind, EventKind::AgentSpawning { .. })
                {
                    got += 1;
                    if got >= 3 {
                        return got;
                    }
                }
            }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let client = Arc::new(NetworkBusClient::with_config(
            Client::new(),
            base,
            "tok".into(),
            "nodeB".into(),
            32,
            16,
            Duration::from_millis(50),
            vec![],
        ));
        let handle = client.clone().spawn_flush_loop();
        for r in ["a", "b", "c"] {
            client.enqueue(make_event(r)).await;
        }
        // 等 tick 触发 flush
        tokio::time::sleep(Duration::from_millis(200)).await;
        client.shutdown(handle).await;

        let got = probe.await.expect("join");
        assert_eq!(got, 3, "三条 event 都该被订阅到");
        srv.abort();
    }

    /// TDD #7：第一次 503 第二次 200 → 整 batch 恰好成功一次（不重复 publish）。
    #[tokio::test]
    async fn flush_retries_on_transient_error() {
        let (fake, base, srv) = spawn_fake_controller(1).await;
        let client = Arc::new(NetworkBusClient::with_config(
            Client::new(),
            base,
            "tok".into(),
            "nodeB".into(),
            32,
            16,
            Duration::from_millis(50),
            vec![20, 20],
        ));
        let handle = client.clone().spawn_flush_loop();
        for r in ["a", "b"] {
            client.enqueue(make_event(r)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        client.shutdown(handle).await;

        assert_eq!(
            fake.success_count.load(Ordering::SeqCst),
            1,
            "503 retry 后只应成功一次，不重复"
        );
        assert_eq!(
            fake.last_batch_len.load(Ordering::SeqCst),
            2,
            "batch 内容完整"
        );
        srv.abort();
    }

    /// TDD #8：所有 retry 全 503 → 放弃整 batch，不成功。
    #[tokio::test]
    async fn flush_drops_batch_after_max_retries() {
        // initial + 2 retry = 3 次尝试全失败
        let (fake, base, srv) = spawn_fake_controller(99).await;
        let client = Arc::new(NetworkBusClient::with_config(
            Client::new(),
            base,
            "tok".into(),
            "nodeB".into(),
            32,
            16,
            Duration::from_millis(50),
            vec![10, 10],
        ));
        let handle = client.clone().spawn_flush_loop();
        client.enqueue(make_event("a")).await;
        // 50ms tick + 3 次尝试 + 2 × 10ms backoff
        tokio::time::sleep(Duration::from_millis(300)).await;
        client.shutdown(handle).await;

        assert_eq!(
            fake.success_count.load(Ordering::SeqCst),
            0,
            "全 503 不该有 success"
        );
        assert!(
            fake.fail_remaining.load(Ordering::SeqCst) <= 99 - 3,
            "至少消耗 3 次失败配额（initial + 2 retry）"
        );
        srv.abort();
    }

    /// TDD #9：shutdown 触发后 flush_loop 把 pending 全 POST 完再退出。
    /// 用 60s tick 隔离掉时间触发，只能走 shutdown 路径——证 shutdown 路径
    /// 自带 flush。
    #[tokio::test]
    async fn shutdown_flushes_pending_before_exit() {
        let (ctrl, base, srv) = spawn_real_controller().await;
        let bus = ctrl.bus().clone();
        let probe = tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut s = bus.subscribe();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            let mut got = 0;
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    return got;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && matches!(ev.kind, EventKind::AgentSpawning { .. })
                {
                    got += 1;
                    if got >= 5 {
                        return got;
                    }
                }
            }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let client = Arc::new(NetworkBusClient::with_config(
            Client::new(),
            base,
            "tok".into(),
            "nodeB".into(),
            32,
            16,
            Duration::from_secs(60),
            vec![],
        ));
        let handle = client.clone().spawn_flush_loop();
        for r in ["a", "b", "c", "d", "e"] {
            client.enqueue(make_event(r)).await;
        }
        client.shutdown(handle).await;

        let got = probe.await.expect("join");
        assert_eq!(got, 5, "shutdown 必须 flush 全部 pending");
        srv.abort();
    }
}
