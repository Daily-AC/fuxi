//! 伏羲 EventBus 集成测试：并发发布者 + 一个长跑订阅者 + 一个迟到订阅者做
//! “先 replay 再 live tail”。
//!
//! 目的：验证常见生产形态下 fan-out、持久化、replay-then-live 的端到端一致性。

use futures_util::StreamExt;
use fuxi_core::{Event, EventKind, EventMeta};
use fuxi_events::{EventBus, ReplayCursor};
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::timeout;

fn mk_event(i: usize) -> Event {
    Event {
        meta: EventMeta::now(),
        kind: EventKind::Custom {
            label: format!("concurrent-{i}"),
            payload: serde_json::json!({"i": i}),
        },
    }
}

fn label(ev: &Event) -> Option<String> {
    match &ev.kind {
        EventKind::Custom { label, .. } => Some(label.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn concurrent_publishers_with_late_replay_subscriber() {
    let bus = EventBus::with_memory_store().await.expect("bus");

    // 长跑订阅者——在任何 publish 之前订阅。
    let long_sub = bus.subscribe();

    let total: usize = 50;
    let publishers: usize = 5;
    let per = total / publishers;

    // 并发 publisher。
    let mut handles = Vec::new();
    for p in 0..publishers {
        let bus_clone = bus.clone();
        handles.push(tokio::spawn(async move {
            for k in 0..per {
                let i = p * per + k;
                bus_clone.publish(mk_event(i)).expect("publish");
                // 轻微让出让 writer 有机会工作；不是强依赖。
                tokio::task::yield_now().await;
            }
        }));
    }
    for h in handles {
        h.await.expect("publisher join");
    }

    // 等写盘 settle——以 replay 读到全部事件为准。
    let mut got_all_persisted = false;
    for _ in 0..50 {
        let hist = bus.replay(ReplayCursor::Beginning, false);
        let items: Vec<_> = hist.collect::<Vec<_>>().await;
        let originals: HashSet<String> = items
            .into_iter()
            .filter_map(|r| r.ok())
            .filter_map(|e| label(&e))
            .filter(|l| l.starts_with("concurrent-"))
            .collect();
        if originals.len() == total {
            got_all_persisted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(got_all_persisted, "所有并发事件最终必须都落库");

    // 长跑订阅者应收到全部（并发语序内集合相等）。
    let mut long_sub = long_sub;
    let mut long_sub_labels: HashSet<String> = HashSet::new();
    for _ in 0..total {
        let Ok(Some(Ok(ev))) = timeout(Duration::from_millis(500), long_sub.next()).await else {
            break;
        };
        if let Some(l) = label(&ev)
            && l.starts_with("concurrent-")
        {
            long_sub_labels.insert(l);
        }
    }
    assert_eq!(long_sub_labels.len(), total, "长跑订阅者应看到全部并发事件");

    // 迟到订阅者：replay 历史 + live tail。发一条新事件验证 live 拼接。
    let bus2 = bus.clone();
    let late_handle = tokio::spawn(async move {
        let stream = bus2.replay(ReplayCursor::Beginning, true);
        let mut stream = stream;
        let mut hist_count = 0usize;
        let mut saw_live = false;
        // 先收至少 `total` 条历史，然后等到一条 live 标签事件。
        while let Ok(Some(item)) = timeout(Duration::from_secs(2), stream.next()).await {
            let Ok(ev) = item else { continue };
            let Some(l) = label(&ev) else { continue };
            if l.starts_with("concurrent-") {
                hist_count += 1;
            }
            if l == "after-live" {
                saw_live = true;
                break;
            }
        }
        (hist_count, saw_live)
    });

    // 给 late subscriber 一点时间开始回放再 publish live 事件。
    tokio::time::sleep(Duration::from_millis(50)).await;
    bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::Custom {
            label: "after-live".into(),
            payload: serde_json::json!({}),
        },
    })
    .expect("publish live");

    let (hist_count, saw_live) = late_handle.await.expect("late join");
    assert!(hist_count >= total, "迟到订阅者回放应覆盖全部历史");
    assert!(saw_live, "迟到订阅者 live_tail 应接收到随后到达的事件");
}
