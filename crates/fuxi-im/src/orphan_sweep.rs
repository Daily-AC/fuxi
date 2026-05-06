//! 启动期 orphan task 兜底——补 v1-session12 §7 #1 的尾巴。
//!
//! ## 真因
//!
//! Bug 3（commit `8a2e03e`）的 `Fuxi::dispatch` pump 退出兜底只对**新派的 task**
//! 生效：rx 关闭 / agent 进程崩 → pump 退出时若 `!saw_terminal` 则 emit
//! TaskCancelled。但**旧 leftover task**（fuxi-im 重启前 / 进程被 kill / agent 异常
//! 退出但 pump 没有跑到那一步）没人触发那个 pump，永远卡 InProgress。
//!
//! 表现：用户 PWA 看到几天前的 task 还在 running 列里，对应门客其实 dead
//! agent_id 找不到（task-fb7437a8 cangjie-extract 撞过）。
//!
//! ## 策略
//!
//! 启动期一次性扫 events.db：对每个 task，取最后事件，若：
//! 1. 最后事件不是 task_state_changed{Done|Cancelled|Delivering}（即非终态）
//! 2. 最后事件时间 < cutoff（默认现在 - 30 分钟）—— 给"刚启动重连还没续上的真活跃
//!    task" 留余量，避免把用户重启 fuxi-im 几秒后看见的非终态错杀
//!
//! 那兜底 publish 一条 `TaskStateChanged{from: 推断, to: Cancelled}`，agent 字段
//! 取最后一条带 agent 的事件，方便审计追溯。
//!
//! ## env 覆盖
//!
//! - `FUXI_ORPHAN_SWEEP_CUTOFF_MINUTES`：cutoff 分钟数（默认 30）
//! - `FUXI_DISABLE_ORPHAN_SWEEP=1`：完全跳过 sweep（CI / 排查用）

use chrono::{Duration, Utc};
use fuxi_core::TaskId;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::task::TaskState;
use fuxi_events::{EventBus, EventStore};
use std::str::FromStr;
use uuid::Uuid;

const DEFAULT_CUTOFF_MINUTES: i64 = 30;

/// sweep 入口——扫所有 task，对 orphan 兜底发 TaskCancelled。返 cancel 条数。
///
/// 不 fail-fast：单 task 处理失败 log warn 跳过，不影响整体启动流程。
pub async fn sweep_orphan_tasks(bus: &EventBus) -> usize {
    if std::env::var_os("FUXI_DISABLE_ORPHAN_SWEEP").is_some() {
        tracing::info!("orphan sweep disabled by FUXI_DISABLE_ORPHAN_SWEEP");
        return 0;
    }
    let cutoff_minutes = std::env::var("FUXI_ORPHAN_SWEEP_CUTOFF_MINUTES")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_CUTOFF_MINUTES);
    let cutoff = Utc::now() - Duration::minutes(cutoff_minutes);

    let store = bus.store();
    let task_ids = match store.list_task_ids().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "orphan sweep: list_task_ids 失败，跳过");
            return 0;
        }
    };
    tracing::info!(
        total_tasks = task_ids.len(),
        cutoff_minutes,
        "orphan sweep 开始"
    );

    let mut cancelled = 0_usize;
    for raw in task_ids {
        match sweep_one(store, bus, &raw, cutoff).await {
            Ok(true) => cancelled += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(task = %raw, error = %e, "orphan sweep: 单 task 处理失败");
            }
        }
    }

    tracing::info!(cancelled, cutoff_minutes, "orphan sweep 完成");
    cancelled
}

/// 处理单 task，返 true 表示发了 cancel。
async fn sweep_one(
    store: &EventStore,
    bus: &EventBus,
    raw_id: &str,
    cutoff: chrono::DateTime<Utc>,
) -> anyhow::Result<bool> {
    let task_id =
        parse_task_id(raw_id).ok_or_else(|| anyhow::anyhow!("非法 task id 格式: {raw_id}"))?;
    let events = store.history_for_task(task_id).await?;
    if events.is_empty() {
        return Ok(false);
    }

    // 最后事件时间——还活跃的 task 不动
    let last_at = events.iter().map(|e| e.meta.at).max().unwrap_or(Utc::now());
    if last_at >= cutoff {
        return Ok(false);
    }

    // 已终态的 task 不动
    let terminal = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::TaskStateChanged { to, .. }
            if matches!(
                to,
                TaskState::Done | TaskState::Cancelled | TaskState::Delivering
            ) =>
        {
            Some(*to)
        }
        _ => None,
    });
    if terminal.is_some() {
        return Ok(false);
    }

    // 推断 from：取最后一条 TaskStateChanged.to 当 from，没有则 InProgress（最常见）
    let from = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::TaskStateChanged { to, .. } => Some(*to),
            _ => None,
        })
        .unwrap_or(TaskState::InProgress);

    // agent：取最后一条带 agent 的事件，便于审计
    let last_agent = events.iter().rev().find_map(|e| e.meta.agent);

    let mut meta = EventMeta::now();
    meta.task = Some(task_id);
    meta.agent = last_agent;
    bus.publish(Event {
        meta,
        kind: EventKind::TaskStateChanged {
            from,
            to: TaskState::Cancelled,
        },
    })
    .map_err(|e| anyhow::anyhow!("publish TaskCancelled 失败: {e}"))?;

    tracing::info!(
        task = %raw_id,
        from = ?from,
        last_agent = ?last_agent,
        last_active = %last_at,
        "orphan sweep: 兜底 cancel"
    );
    Ok(true)
}

fn parse_task_id(raw: &str) -> Option<TaskId> {
    let trimmed = raw.strip_prefix("task-").unwrap_or(raw);
    Uuid::from_str(trimmed).ok().map(TaskId::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::id::AgentId;

    /// publish 一条带 task / agent / kind 的事件，at 显式指定方便测时间锚。
    async fn pub_at(
        bus: &EventBus,
        task: TaskId,
        agent: Option<AgentId>,
        kind: EventKind,
        at: chrono::DateTime<Utc>,
    ) {
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        meta.agent = agent;
        meta.at = at;
        bus.publish(Event { meta, kind }).expect("publish");
        // 给 store 一点持久化时间——bus.publish 是 fire-and-forget
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn fresh_task_under_cutoff_not_swept() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let task = TaskId::new();
        let now = Utc::now();
        pub_at(
            &bus,
            task,
            None,
            EventKind::TaskCreated {
                title: "fresh".into(),
                description: "x".into(),
            },
            now - Duration::minutes(2),
        )
        .await;
        // 默认 30 分 cutoff，2 分钟前不该被扫
        let n = sweep_orphan_tasks(&bus).await;
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn orphan_task_over_cutoff_gets_cancelled() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let task = TaskId::new();
        let agent = AgentId::new();
        let stale = Utc::now() - Duration::hours(2);
        pub_at(
            &bus,
            task,
            Some(agent),
            EventKind::TaskCreated {
                title: "orphan".into(),
                description: "x".into(),
            },
            stale,
        )
        .await;
        pub_at(
            &bus,
            task,
            Some(agent),
            EventKind::TaskDispatched { to: agent },
            stale,
        )
        .await;

        let mut sub = bus.subscribe();
        let n = sweep_orphan_tasks(&bus).await;
        assert_eq!(n, 1, "应兜底 cancel 一条");

        // 验证 publish 出来的就是 TaskCancelled
        use futures_util::StreamExt;
        let saw = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            loop {
                let Some(Ok(ev)) = sub.next().await else {
                    return None;
                };
                if ev.meta.task == Some(task)
                    && matches!(
                        &ev.kind,
                        EventKind::TaskStateChanged {
                            to: TaskState::Cancelled,
                            ..
                        }
                    )
                {
                    return Some(ev);
                }
            }
        })
        .await;
        assert!(
            saw.is_ok() && saw.unwrap().is_some(),
            "应订阅到 TaskCancelled"
        );
    }

    #[tokio::test]
    async fn done_task_not_re_cancelled() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let task = TaskId::new();
        let stale = Utc::now() - Duration::hours(2);
        pub_at(
            &bus,
            task,
            None,
            EventKind::TaskCreated {
                title: "done".into(),
                description: "x".into(),
            },
            stale,
        )
        .await;
        pub_at(
            &bus,
            task,
            None,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
            stale,
        )
        .await;

        let n = sweep_orphan_tasks(&bus).await;
        assert_eq!(n, 0, "已 Done 的 task 不该再 cancel");
    }

    #[tokio::test]
    async fn cancelled_task_not_re_cancelled() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let task = TaskId::new();
        let stale = Utc::now() - Duration::hours(2);
        pub_at(
            &bus,
            task,
            None,
            EventKind::TaskCreated {
                title: "cancel".into(),
                description: "x".into(),
            },
            stale,
        )
        .await;
        pub_at(
            &bus,
            task,
            None,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Cancelled,
            },
            stale,
        )
        .await;

        let n = sweep_orphan_tasks(&bus).await;
        assert_eq!(n, 0, "已 Cancelled 的 task 不该重复 cancel");
    }

    /// env 关掉时直接返 0，不扫不发——CI / 排查场景。
    #[tokio::test]
    async fn env_disable_short_circuits() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let task = TaskId::new();
        let stale = Utc::now() - Duration::hours(2);
        pub_at(
            &bus,
            task,
            None,
            EventKind::TaskCreated {
                title: "x".into(),
                description: "x".into(),
            },
            stale,
        )
        .await;

        // SAFETY：单测内独占。其他并发测不读 FUXI_DISABLE_ORPHAN_SWEEP。
        unsafe { std::env::set_var("FUXI_DISABLE_ORPHAN_SWEEP", "1") };
        let n = sweep_orphan_tasks(&bus).await;
        unsafe { std::env::remove_var("FUXI_DISABLE_ORPHAN_SWEEP") };
        assert_eq!(n, 0);
    }

    /// 多 task 混合：fresh + orphan + done → 只 orphan 被 cancel。
    #[tokio::test]
    async fn mixed_tasks_only_orphans_cancelled() {
        let bus = EventBus::with_memory_store().await.unwrap();
        let stale = Utc::now() - Duration::hours(2);
        let now = Utc::now();

        // fresh：当前活跃
        let fresh = TaskId::new();
        pub_at(
            &bus,
            fresh,
            None,
            EventKind::TaskCreated {
                title: "f".into(),
                description: "x".into(),
            },
            now - Duration::minutes(1),
        )
        .await;

        // orphan: 老 + 非终态
        let orphan = TaskId::new();
        pub_at(
            &bus,
            orphan,
            None,
            EventKind::TaskCreated {
                title: "o".into(),
                description: "x".into(),
            },
            stale,
        )
        .await;

        // done: 老 + 终态
        let done = TaskId::new();
        pub_at(
            &bus,
            done,
            None,
            EventKind::TaskCreated {
                title: "d".into(),
                description: "x".into(),
            },
            stale,
        )
        .await;
        pub_at(
            &bus,
            done,
            None,
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
            stale,
        )
        .await;

        let n = sweep_orphan_tasks(&bus).await;
        assert_eq!(n, 1, "应只 cancel orphan 一条");
    }
}
