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

use crate::push::notify::{PushPayload, PushSender, notify};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use fuxi_core::event::{Event, EventKind};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_events::EventBus;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// 玄女 idle 推送的等待窗口——超过即视为"在等用户"。
pub const XUANNV_IDLE_WINDOW: Duration = Duration::from_secs(30);

/// 同一 task done 的 dedup TTL——60s 内不重推。
pub const TASK_DONE_DEDUP_TTL: Duration = Duration::from_secs(60);

/// 启动钩子任务——返回 JoinHandle 以便测试 awati / 生产 abort。
///
/// `pool`：通知子系统持的 im.db pool。
/// `sender`：fan-out 实装（生产用 [`super::notify::HyperPushSender`]，
/// 测试注 mock）。
/// `bus`：EventBus 句柄（玄女订阅源）。
/// `xuannv_id`：玄女 AgentId——只关心她的 AgentResponded。
pub fn spawn(
    pool: SqlitePool,
    sender: Arc<dyn PushSender>,
    bus: EventBus,
    xuannv_id: AgentId,
) -> JoinHandle<()> {
    spawn_with_windows(
        pool,
        sender,
        bus,
        xuannv_id,
        XUANNV_IDLE_WINDOW,
        TASK_DONE_DEDUP_TTL,
    )
}

/// 测试用入口——注入更短的 window 以加速 idle / dedup 测试。
pub fn spawn_with_windows(
    pool: SqlitePool,
    sender: Arc<dyn PushSender>,
    bus: EventBus,
    xuannv_id: AgentId,
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
                &state,
                xuannv_id,
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

async fn handle_event(
    pool: &SqlitePool,
    sender: Arc<dyn PushSender>,
    state: &Arc<Mutex<HookState>>,
    xuannv_id: AgentId,
    idle_window: Duration,
    dedup_ttl: Duration,
    ev: Event,
) {
    match ev.kind {
        EventKind::AgentResponded { .. } => {
            // 仅当来自玄女才计 idle。门客响应是另一回事——他们不直接面向用户。
            if ev.meta.agent != Some(xuannv_id) {
                return;
            }
            let mut s = state.lock().await;
            // 替换旧 timer：每条新响应都重新计时。
            if let Some(old) = s.xuannv_idle_timer.take() {
                old.abort();
            }
            let pool2 = pool.clone();
            let sender2 = sender.clone();
            let h = tokio::spawn(async move {
                tokio::time::sleep(idle_window).await;
                let payload = PushPayload::new("玄女在等你", "她已停下，等你回话", "/#/conv");
                if let Err(e) = notify(&pool2, sender2, &payload).await {
                    tracing::warn!(error = %e, "玄女 idle 推送失败");
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
                tracing::warn!(error = %e, %task_id, "task done 推送失败");
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

    async fn fresh_pool() -> SqlitePool {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        // 至少一条订阅，notify 才有 fan-out 目标
        upsert(&pool, "d1", "https://e/1", "k", "a").await.unwrap();
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
        let xuannv = AgentId::new();
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            bus.clone(),
            xuannv,
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
            },
        })
        .expect("publish");

        wait_pushes(&sender, 1, 500).await;
        let pushes = sender.snapshot();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].title, "玄女在等你");
    }

    /// 玄女响应后立刻 user 开口 → idle 推送被取消。
    #[tokio::test]
    async fn user_prompt_cancels_idle_timer() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let xuannv = AgentId::new();
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            bus.clone(),
            xuannv,
            Duration::from_millis(80),
            Duration::from_secs(60),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta1 = EventMeta::now();
        meta1.agent = Some(xuannv);
        bus.publish(Event {
            meta: meta1,
            kind: EventKind::AgentResponded { text: "嗯".into() },
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
    }

    /// 门客 AgentResponded 不计 idle（公理：只关心玄女对用户的输出）。
    #[tokio::test]
    async fn worker_response_does_not_trigger_idle() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            bus.clone(),
            xuannv,
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
            },
        })
        .unwrap();

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(sender.snapshot().is_empty(), "门客响应不该触发 玄女idle");
    }

    /// task → Done 触发"任务 N 完成"一条。
    #[tokio::test]
    async fn task_done_triggers_notification() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let xuannv = AgentId::new();
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            bus.clone(),
            xuannv,
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
    }

    /// 同一 task done 60s（这里短 ttl）内不重推。
    #[tokio::test]
    async fn task_done_dedup_within_ttl() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let xuannv = AgentId::new();
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            bus.clone(),
            xuannv,
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
    }

    /// 不同 task_id 各自推。
    #[tokio::test]
    async fn task_done_different_ids_each_push() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let pool = fresh_pool().await;
        let sender = Arc::new(CountingSender::default());
        let xuannv = AgentId::new();
        let _h = spawn_with_windows(
            pool,
            sender.clone() as Arc<dyn PushSender>,
            bus.clone(),
            xuannv,
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
    }
}
