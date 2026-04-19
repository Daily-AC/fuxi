//! fuxi-scheduler · gated E2E
//!
//! 运行： `FUXI_RUN_SCHED_E2E=1 cargo test -p fuxi-scheduler --test e2e_scheduler`
//!
//! 语义：端到端地验证更漏—— TriggerStore + Keeper + EventBus 三者组合起来，
//! 每 3 秒 fire 一次的 cron trigger，在 7 秒内至少 fire 2 次。
//!
//! 为什么 gated：tokio 真 timer 抖动在 CI 上可能不稳；默认 TDD 集成测试走假时钟，
//! 这一条留给开发者手动自测「真的一秒一秒在转」。

use chrono::Utc;
use futures_util::StreamExt;
use fuxi_core::EventKind;
use fuxi_events::EventBus;
use fuxi_scheduler::keeper::{Keeper, SystemClock};
use fuxi_scheduler::spec::TriggerSpec;
use fuxi_scheduler::store::NewTrigger;
use fuxi_scheduler::{TriggerStore, new_trigger_id};
use std::sync::Arc;
use std::time::Duration;

fn e2e_enabled() -> bool {
    std::env::var("FUXI_RUN_SCHED_E2E").is_ok()
}

#[tokio::test]
async fn every_three_seconds_fires_at_least_twice_in_7s() {
    if !e2e_enabled() {
        eprintln!("skip: set FUXI_RUN_SCHED_E2E=1 to run");
        return;
    }

    let store = TriggerStore::connect_memory().await.expect("store");
    let bus = EventBus::with_memory_store().await.expect("bus");

    // `*/3 * * * * *` 每 3 秒一次（秒级）。注册后从 created_at 起算下一 tick。
    // 为了让 7 秒窗口稳定拿到 >=2，我们等一拍再起 Keeper：
    let now = Utc::now();
    let _row = store
        .insert(NewTrigger {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "*/3 * * * * *".into(),
                tz: None,
            },
            intent: "say hi".into(),
            session_id: None,
            max_failures: None,
        })
        .await
        .expect("insert");

    let mut sub = bus.subscribe();
    let keeper_task = Arc::new(Keeper::new(
        store.clone(),
        bus.clone(),
        Arc::new(SystemClock),
    ))
    .spawn();

    // 等 8 秒；收集 TriggerFired cause=scheduled 事件
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut fires = 0usize;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, sub.next()).await {
            Ok(Some(Ok(ev))) => {
                if let EventKind::TriggerFired { cause, .. } = ev.kind
                    && cause == "scheduled"
                {
                    fires += 1;
                }
            }
            _ => break,
        }
    }
    keeper_task.abort();
    let elapsed_secs = 8;
    eprintln!("elapsed={elapsed_secs}s from now={now}; scheduled fires = {fires}");
    assert!(
        fires >= 2,
        "期望 7+s 内至少 2 次 TriggerFired，实际 {fires}"
    );
}
