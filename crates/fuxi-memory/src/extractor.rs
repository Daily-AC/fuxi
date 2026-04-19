//! 简册 → 甲骨/河图的抽取管线（v1 stub）。
//!
//! v1 只做"骨架 + 订阅"——真正的 fact 抽取走 A2A 门客形式是 v2 完善
//! （`docs/architecture-v1.md §M1.1`）。这里保证：
//! - 订阅了 EventBus 的 `TaskStateChanged { to: Done }` 和 `AgentResponded`
//! - 任务结束后异步扫该 task 的事件，暂时只日志、**不**自动落库
//!
//! 留 TODO 给 v2：在 `extract_from_task` 里 spawn 一个 extractor 门客
//! （新 role），把它的输出（JSON fact list）逐条写入 `OracleStore`。

use crate::oracle::OracleStore;
use futures_util::StreamExt;
use fuxi_core::{Event, EventKind, TaskId, TaskState};
use fuxi_events::EventBus;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Extractor 行为开关。默认关自动落库（v1 还没真正的抽取器）。
#[derive(Debug, Clone, Default)]
pub struct ExtractorConfig {
    /// 自动落库开关——v1 默认 `false`，走观察模式。
    pub auto_persist: bool,
}

/// 订阅 EventBus、在任务 Done 时触发抽取流程。
///
/// 被调用方 `spawn` 后长跑；drop Extractor 会自然关闭 join handle（broadcast
/// subscriber 断掉导致 stream end）。
pub struct Extractor {
    _handle: JoinHandle<()>,
}

impl Extractor {
    /// 起一个后台 task 订阅 bus。调用方在 drop `Extractor` 时停止。
    pub fn spawn(bus: EventBus, oracle: Arc<OracleStore>, cfg: ExtractorConfig) -> Self {
        let mut stream = bus.subscribe();
        let handle = tokio::spawn(async move {
            info!(auto_persist = cfg.auto_persist, "extractor: 开始订阅简册");
            while let Some(item) = stream.next().await {
                let Ok(ev) = item else { continue };
                if let Some(task_id) = task_done_trigger(&ev) {
                    debug!(%task_id, "extractor: 观察到 TaskStateChanged → Done");
                    // TODO(v2): spawn A2A extractor 门客，读取 task_id 对应的事件流，
                    //           抽 fact 后调 OracleStore::insert。当前只 stub。
                    if cfg.auto_persist {
                        // 故意留空——挂点在这里，方便以后接上真正的 extractor 门客。
                        let _ = &oracle;
                        warn!(%task_id, "extractor: auto_persist=true 但尚未接入 v2 抽取器");
                    }
                }
            }
            info!("extractor: 简册订阅结束，退出");
        });
        Self { _handle: handle }
    }
}

/// `ev` 若是 "任务落至 Done" 的状态变更事件、返回该 task_id。
fn task_done_trigger(ev: &Event) -> Option<TaskId> {
    match &ev.kind {
        EventKind::TaskStateChanged {
            to: TaskState::Done,
            ..
        } => ev.meta.task,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::{EventMeta, TaskId};

    fn done_event(task: TaskId) -> Event {
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        }
    }

    #[test]
    fn task_done_trigger_fires_only_on_done() {
        let task = TaskId::new();
        let ev = done_event(task);
        assert_eq!(task_done_trigger(&ev), Some(task));

        let mut meta = EventMeta::now();
        meta.task = Some(task);
        let other = Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Delivering,
            },
        };
        assert!(task_done_trigger(&other).is_none());
    }
}
