//! 订阅 EventBus → 触发 [`super::notify::notify`] 的钩子。
//!
//! 两个触发口：
//!
//! 1. **玄女 idle 等用户回复 >30s** —— 看到玄女 `AgentResponded` 后启动 30s
//!    定时器；到点前没收到新 `UserPrompted` 就推一条 "玄女在等你"。
//!    途中再来玄女响应 → 重置计时器（用最新一条算）。
//! 2. **任务完成** —— `TaskStateChanged{to: Done}` 推 "任务 #短id 完成"。
//!    简化：当前 fuxi-core 没有 parent/root 概念，所有 task done 都推；
//!    60s 内同 task_id 去重。TODO: task tree API 暴露 is_root 后再过滤。
//!
//! 公理 #3 真实时——纯 `bus.subscribe()` 推送，无 polling。
//! 公理 #5 SQLite 单一真相——hooks 不持自己的状态表，dedup map 是
//! in-mem，进程重启即清（合理：首启没人订阅 push）。
//!
//! WHY 两路一起推：自 `feat/fuxi-im-android` 起，每个触发点同时 fan-out
//! Web Push（[`notify`]）和 FCM（[`notify_fcm`]）——PWA 用户和原生 Android
//! app 用户各走各的通道，hooks 不区分。任一路失败只 log，不影响另一路。

use crate::push::fcm::{FcmPusher, notify_fcm};
use crate::push::notify::{PushPayload, PushSender, notify};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use fuxi_core::TopicId;
use fuxi_core::event::{Event, EventKind};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_events::EventBus;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

/// 玄女分身池快照——`Fuxi::xuannv_pool_watch()` 的 Receiver 类型别名。
pub type XuannvPoolWatch = watch::Receiver<HashMap<TopicId, AgentId>>;

/// 玄女 idle 推送的等待窗口——超过即视为"在等用户"。
pub const XUANNV_IDLE_WINDOW: Duration = Duration::from_secs(30);

/// 同一 task done 的 dedup TTL——60s 内不重推。
pub const TASK_DONE_DEDUP_TTL: Duration = Duration::from_secs(60);

/// 启动钩子任务——返回 JoinHandle 以便测试 awati / 生产 abort。
///
/// `pool`：通知子系统持的 im.db pool。
/// `sender`：Web Push fan-out 实装（生产用 [`super::notify::HyperPushSender`]，
/// 测试注 mock）。
/// `fcm_sender`：FCM fan-out 实装（生产用 [`super::fcm::HttpFcmSender`]，
/// 无凭据时退化 [`super::fcm::NoopFcmSender`]；测试注 mock）。
/// `bus`：EventBus 句柄（玄女订阅源）。
/// `xuannv_watch`：玄女分身池 watch——过滤 AgentResponded 时**每帧实时读**当前
/// 全部活分身 id。绝不能 snapshot 一个 AgentId 烤进闭包：玄女 id 会在会话中
/// 漂移（topic 切换 handoff / idle GC 重生 / 服务重启 fresh session），烤死的
/// 旧 id 永不匹配 → 「玄女在等你」推送静默死掉（2026-06-10 用户实测命中；
/// 同 conv WS snapshot bug，见 memory `feedback_dynamic_agent_id_via_watch`）。
pub fn spawn(
    pool: SqlitePool,
    sender: Arc<dyn PushSender>,
    fcm_sender: Arc<dyn FcmPusher>,
    bus: EventBus,
    xuannv_watch: XuannvPoolWatch,
) -> JoinHandle<()> {
    spawn_with_windows(
        pool,
        sender,
        fcm_sender,
        bus,
        xuannv_watch,
        XUANNV_IDLE_WINDOW,
        TASK_DONE_DEDUP_TTL,
    )
}

/// 测试用入口——注入更短的 window 以加速 idle / dedup 测试。
pub fn spawn_with_windows(
    pool: SqlitePool,
    sender: Arc<dyn PushSender>,
    fcm_sender: Arc<dyn FcmPusher>,
    bus: EventBus,
    xuannv_watch: XuannvPoolWatch,
    idle_window: Duration,
    dedup_ttl: Duration,
) -> JoinHandle<()> {
    let state: Arc<Mutex<HookState>> = Arc::new(Mutex::new(HookState::default()));
    let mut sub = bus.subscribe();
    tokio::spawn(async move {
        while let Some(item) = sub.next().await {
            let Ok(ev) = item else {
                continue; // Lagged / 错误事件——继续，不打断订阅链
            };
            handle_event(
                &pool,
                sender.clone(),
                fcm_sender.clone(),
                &state,
                &xuannv_watch,
                idle_window,
                dedup_ttl,
                ev,
            )
            .await;
        }
        tracing::debug!("push hooks: subscribe stream 结束，退出");
    })
}

/// 钩子内部累计状态——in-mem，进程重启即清。
#[derive(Default)]
struct HookState {
    /// 当前活动的"玄女 idle"定时器（每来一次玄女 AgentResponded 就替换掉旧的）。
    /// `Option<JoinHandle<()>>` 的 None 表示此刻无活动定时器。
    xuannv_idle_timer: Option<JoinHandle<()>>,
    /// task done 去重表：(task_id → 上次推送时刻)。超过 dedup_ttl 即可再推。
    task_done_last_at: HashMap<TaskId, DateTime<Utc>>,
}

#[allow(clippy::too_many_arguments)] // 两路 sender + 两个 window 注入——拆 struct 收益不抵可读性损失
async fn handle_event(
    pool: &SqlitePool,
    sender: Arc<dyn PushSender>,
    fcm_sender: Arc<dyn FcmPusher>,
    state: &Arc<Mutex<HookState>>,
    xuannv_watch: &XuannvPoolWatch,
    idle_window: Duration,
    dedup_ttl: Duration,
    ev: Event,
) {
    match ev.kind {
        EventKind::AgentResponded { .. } => {
            // 仅当来自玄女（任一活跃分身）才计 idle。门客响应是另一回事——
            // 他们不直接面向用户。实时读 watch 跟随 id 漂移，见 spawn doc。
            let is_xuannv = ev
                .meta
                .agent
                .is_some_and(|a| xuannv_watch.borrow().values().any(|v| *v == a));
            if !is_xuannv {
                return;
            }
            let mut s = state.lock().await;
            // 替换旧 timer：每条新响应都重新计时。
            if let Some(old) = s.xuannv_idle_timer.take() {
                old.abort();
            }
            let pool2 = pool.clone();
            let sender2 = sender.clone();
            let fcm_sender2 = fcm_sender.clone();
            let h = tokio::spawn(async move {
                tokio::time::sleep(idle_window).await;
                let payload = PushPayload::new("玄女在等你", "她已停下，等你回话", "/#/conv");
                if let Err(e) = notify(&pool2, sender2, &payload).await {
                    tracing::warn!(error = %e, "玄女 idle Web Push 推送失败");
                }
                if let Err(e) = notify_fcm(&pool2, fcm_sender2, &payload).await {
                    tracing::warn!(error = %e, "玄女 idle FCM 推送失败");
                }
            });
            s.xuannv_idle_timer = Some(h);
        }
        EventKind::UserPrompted { .. } => {
            // 用户开口 → 取消"玄女在等你"——再等就是又开了一轮。
            let mut s = state.lock().await;
            if let Some(old) = s.xuannv_idle_timer.take() {
                old.abort();
            }
        }
        EventKind::TaskStateChanged {
            to: fuxi_core::task::TaskState::Done,
            ..
        } => {
            let Some(task_id) = ev.meta.task else {
                return; // 没 task id 推不出可操作 link
            };
            let now = Utc::now();
            // dedup：60s 内同 task_id 不重推
            {
                let mut s = state.lock().await;
                if let Some(last) = s.task_done_last_at.get(&task_id)
                    && now.signed_duration_since(*last)
                        < chrono::Duration::from_std(dedup_ttl)
                            .unwrap_or(chrono::Duration::seconds(60))
                {
                    return;
                }
                s.task_done_last_at.insert(task_id, now);
            }
            let short = short_task_id(&task_id);
            let payload = PushPayload::new(
                format!("任务 {short} 完成"),
                "点击查看",
                format!("/#/task/{task_id}"),
            );
            if let Err(e) = notify(pool, sender, &payload).await {
                tracing::warn!(error = %e, %task_id, "task done Web Push 推送失败");
            }
            if let Err(e) = notify_fcm(pool, fcm_sender, &payload).await {
                tracing::warn!(error = %e, %task_id, "task done FCM 推送失败");
            }
        }
        _ => {}
    }
}

fn short_task_id(id: &TaskId) -> String {
    let s = id.to_string();
    // 形如 "task_abcdef..." → 取末 8 位足以唯一标识 + 在通知里好读
    let len = s.len();
    if len > 8 { s[len - 8..].to_string() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use crate::error::Result;
    use crate::push::fcm::{FcmSendOutcome, upsert_token};
    use crate::push::store::upsert;
    use async_trait::async_trait;
    use fuxi_core::event::{EventKind, EventMeta};
    use fuxi_core::task::TaskState;
    use std::sync::Mutex as StdMutex;
    use tempfile::tempdir;

    #[derive(Default)]
    struct CountingSender {
        sent: StdMutex<Vec<PushPayload>>,
    }

    #[async_trait]
    impl PushSender for CountingSender {
        async fn send_one(
            &self,
            _sub: &crate::push::store::PushSubscriptionRow,
            payload_json: &[u8],
        ) -> Result<()> {
            let p: serde_json::Value = serde_json::from_slice(payload_json).unwrap();
            self.sent.lock().unwrap().push(PushPayload::new(
                p["title"].as_str().unwrap_or_default(),
                p["body"].as_str().unwrap_or_default(),
                p["url"].as_str().unwrap_or_default(),
            ));
            Ok(())
        }
    }

    impl CountingSender {
        fn snapshot(&self) -> Vec<PushPayload> {
            self.sent.lock().unwrap().clone()
        }
    }

    /// FCM 侧的计数 sender——仿 [`CountingSender`]，验证 hooks 两路都推。
    #[derive(Default)]
    struct CountingFcmSender {
        sent: StdMutex<Vec<PushPayload>>,
    }

    #[async_trait]
    impl FcmPusher for CountingFcmSender {
        async fn send_one(&self, _token: &str, payload: &PushPayload) -> Result<FcmSendOutcome> {
            self.sent.lock().unwrap().push(payload.clone());
            Ok(FcmSendOutcome::Delivered)
        }
    }

    impl CountingFcmSender {
        fn snapshot(&self) -> Vec<PushPayload> {
            self.sent.lock().unwrap().clone()
        }
    }

    /// 单 id 包成分身池 watch——多数测试只需要一个固定玄女。
    fn watch_of(id: AgentId) -> (watch::Sender<HashMap<TopicId, AgentId>>, XuannvPoolWatch) {
        let (tx, rx) = watch::channel(HashMap::from([(TopicId::general(), id)]));
        (tx, rx)
    }

    async fn fresh_pool() -> SqlitePool {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        // 至少一条 Web Push 订阅 + 一条 FCM token，两路 notify 才有 fan-out 目标
        upsert(&pool, "d1", "https://e/1", "k", "a").await.unwrap();
        upsert_token(&pool, "fcm-tok-1", "Pixel").await.unwrap();
        std::mem::forget(dir);
        pool
    }

    /// 等钩子收到 N 条 push 或超时 panic。
    async fn wait_pushes(sender: &Arc<CountingSender>, at_least: usize, max_ms: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(max_ms);
        loop {
            if sender.snapshot().len() >= at_least {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "等 push 至少 {} 条超时（实收 {}）",
                    at_least,
                    sender.snapshot().len()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 玄女发响应后等过 idle window → 触发 "玄女在等你" 一条。
    #[tokio::test]
    async fn xuannv_idle_triggers_notification() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let xuannv = AgentId::new();
        let (_tx, xuannv_rx) = watch_of(xuannv);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_millis(50), // idle window
            Duration::from_secs(60),   // task done dedup
        );
        // 给订阅一点时间 settle
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(xuannv);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentResponded {
                text: "好的".into(),
                artifact_ref: None,
            },
        })
        .expect("publish");

        wait_pushes(&sender, 1, 500).await;
        let pushes = sender.snapshot();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].title, "玄女在等你");
        // 同一触发也应 fan-out 到 FCM 那路。
        assert_eq!(fcm.snapshot().len(), 1, "FCM 也应收到玄女 idle 推送");
        assert_eq!(fcm.snapshot()[0].title, "玄女在等你");
    }

    /// 回归门禁（2026-06-10 用户实测 bug）：玄女 id 漂移后（handoff/重生换新 id），
    /// 新 id 的 AgentResponded 仍要触发推送——hooks 必须实时读 watch，不能 snapshot。
    #[tokio::test]
    async fn xuannv_id_drift_still_triggers_notification() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let old_id = AgentId::new();
        let (tx, xuannv_rx) = watch_of(old_id);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_millis(50),
            Duration::from_secs(60),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 模拟 handoff：分身池换成新 id（老 id 退场）
        let new_id = AgentId::new();
        tx.send(HashMap::from([(TopicId::general(), new_id)]))
            .expect("watch send");

        // 老 id 的响应不再属于玄女——不触发
        let mut meta_old = EventMeta::now();
        meta_old.agent = Some(old_id);
        bus.publish(Event {
            meta: meta_old,
            kind: EventKind::AgentResponded {
                text: "stale".into(),
                artifact_ref: None,
            },
        })
        .unwrap();

        // 新 id 的响应必须触发
        let mut meta_new = EventMeta::now();
        meta_new.agent = Some(new_id);
        bus.publish(Event {
            meta: meta_new,
            kind: EventKind::AgentResponded {
                text: "我回来了".into(),
                artifact_ref: None,
            },
        })
        .unwrap();

        wait_pushes(&sender, 1, 500).await;
        let pushes = sender.snapshot();
        assert_eq!(pushes.len(), 1, "漂移后新 id 应触发恰好一条");
        assert_eq!(pushes[0].title, "玄女在等你");
        assert_eq!(fcm.snapshot().len(), 1);
    }

    /// 玄女响应后立刻 user 开口 → idle 推送被取消。
    #[tokio::test]
    async fn user_prompt_cancels_idle_timer() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let xuannv = AgentId::new();
        let (_tx, xuannv_rx) = watch_of(xuannv);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_millis(80),
            Duration::from_secs(60),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta1 = EventMeta::now();
        meta1.agent = Some(xuannv);
        bus.publish(Event {
            meta: meta1,
            kind: EventKind::AgentResponded {
                text: "嗯".into(),
                artifact_ref: None,
            },
        })
        .unwrap();

        // 在 idle window 内用户开口
        tokio::time::sleep(Duration::from_millis(20)).await;
        bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::UserPrompted {
                text: "继续".into(),
            },
        })
        .unwrap();

        // 等 idle window 整个过去
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            sender.snapshot().is_empty(),
            "用户开口后不应触发 idle 推送，实推 {:?}",
            sender.snapshot()
        );
        assert!(fcm.snapshot().is_empty(), "用户开口后 FCM 那路也不该推");
    }

    /// 门客 AgentResponded 不计 idle（公理：只关心玄女对用户的输出）。
    #[tokio::test]
    async fn worker_response_does_not_trigger_idle() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let (_tx, xuannv_rx) = watch_of(xuannv);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_millis(50),
            Duration::from_secs(60),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentResponded {
                text: "我做完了".into(),
                artifact_ref: None,
            },
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(sender.snapshot().is_empty(), "门客响应不该触发 玄女idle");
        assert!(fcm.snapshot().is_empty(), "门客响应 FCM 那路也不该推");
    }

    /// task → Done 触发"任务 N 完成"一条。
    #[tokio::test]
    async fn task_done_triggers_notification() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let xuannv = AgentId::new();
        let (_tx, xuannv_rx) = watch_of(xuannv);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_secs(60),
            Duration::from_secs(60),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        })
        .unwrap();

        wait_pushes(&sender, 1, 500).await;
        let pushes = sender.snapshot();
        assert_eq!(pushes.len(), 1);
        assert!(
            pushes[0].title.starts_with("任务 "),
            "title: {}",
            pushes[0].title
        );
        assert!(pushes[0].url.contains(&task.to_string()), "url 含 task id");
        // task done 也应 fan-out 到 FCM。
        let fcm_pushes = fcm.snapshot();
        assert_eq!(fcm_pushes.len(), 1, "FCM 也应收到 task done 推送");
        assert!(fcm_pushes[0].title.starts_with("任务 "));
    }

    /// 同一 task done 60s（这里短 ttl）内不重推。
    #[tokio::test]
    async fn task_done_dedup_within_ttl() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let xuannv = AgentId::new();
        let (_tx, xuannv_rx) = watch_of(xuannv);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_secs(60),
            Duration::from_millis(200), // 短 dedup 让测试快
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let task = TaskId::new();
        for _ in 0..3 {
            let mut meta = EventMeta::now();
            meta.task = Some(task);
            bus.publish(Event {
                meta,
                kind: EventKind::TaskStateChanged {
                    from: TaskState::Delivering,
                    to: TaskState::Done,
                },
            })
            .unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        // 应只 1 条（dedup 200ms 期内三条折一）
        tokio::time::sleep(Duration::from_millis(80)).await;
        let pushes = sender.snapshot();
        assert_eq!(pushes.len(), 1, "60s 内 dedup 同 task_id：实推 {pushes:?}");
        // dedup 在 notify 之前——FCM 那路同样只推 1 条。
        assert_eq!(fcm.snapshot().len(), 1, "dedup 对 FCM 路也生效");
    }

    /// 不同 task_id 各自推。
    #[tokio::test]
    async fn task_done_different_ids_each_push() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let fcm = Arc::new(CountingFcmSender::default());
        let xuannv = AgentId::new();
        let (_tx, xuannv_rx) = watch_of(xuannv);
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            fcm.clone() as Arc<dyn FcmPusher>,
            bus.clone(),
            xuannv_rx,
            Duration::from_secs(60),
            Duration::from_secs(60),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        for _ in 0..3 {
            let task = TaskId::new();
            let mut meta = EventMeta::now();
            meta.task = Some(task);
            bus.publish(Event {
                meta,
                kind: EventKind::TaskStateChanged {
                    from: TaskState::Delivering,
                    to: TaskState::Done,
                },
            })
            .unwrap();
        }

        wait_pushes(&sender, 3, 500).await;
        assert_eq!(sender.snapshot().len(), 3);
        // FCM 那路也各推一次。
        assert_eq!(fcm.snapshot().len(), 3, "三个不同 task FCM 各推一次");
    }
}
