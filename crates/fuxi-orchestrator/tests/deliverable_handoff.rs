//! B1 deliverable handoff e2e —— Decision 13 全链路验证。
//!
//! 三测试与 task #6 description 一一对应：
//! 1. middle_events_dont_trigger_xuannv_attention —— 中间事件 silent，公理 2 仍可查
//! 2. deliverable_review_triggers_xuannv_attention —— AgentRequestReview 唯一触发 attention
//! 3. review_timeout_falls_back_to_event —— retry 全失败时 publish ReviewRequestTimeout（依赖 β #4）
//!
//! 选择独立 test file（不污染 dispatch.rs 1402 行）：B1 是新引入的子系统，
//! e2e 与既有 dispatch / intervene 解耦，未来加 case 也只动这一个 file。

use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_core::DeliverableKind;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::trigger_lookup::TriggerLookup;
use fuxi_events::{EventBus, ReplayCursor};
use fuxi_orchestrator::bridge::{Intervener, SystemEventBridge};
use fuxi_orchestrator::{OrchestratorError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

// ── shared mocks ───────────────────────────────────────────────

/// 控制 `MockIntervener::intervene` 的回值，让我们能模拟"玄女接不到"。
/// `AlwaysErr` 用于 timeout 兜底测试；`ErrThenOk` 预留给后续 retry 部分恢复测试。
#[derive(Default, Clone, Copy)]
enum FailMode {
    #[default]
    Ok,
    AlwaysErr,
    /// 前 N 次 Err，第 N+1 次 Ok。
    #[allow(dead_code)] // 预留给后续 retry 部分恢复测试
    ErrThenOk(usize),
}

#[derive(Default)]
struct MockIntervener {
    calls: Mutex<Vec<(AgentId, bool, String)>>,
    roles: Mutex<HashMap<AgentId, String>>,
    fail_mode: Mutex<FailMode>,
    /// `ErrThenOk(N)` 进度计数：每次调用 +1，命中 >N 才返 Ok。
    err_then_ok_count: Mutex<usize>,
}

impl MockIntervener {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    async fn set_role(&self, id: AgentId, role: &str) {
        self.roles.lock().await.insert(id, role.to_string());
    }

    async fn snapshot(&self) -> Vec<(AgentId, bool, String)> {
        self.calls.lock().await.clone()
    }

    async fn set_fail(&self, mode: FailMode) {
        *self.fail_mode.lock().await = mode;
        *self.err_then_ok_count.lock().await = 0;
    }
}

#[async_trait]
impl Intervener for MockIntervener {
    async fn intervene(&self, agent_id: AgentId, interrupt: bool, text: &str) -> Result<()> {
        let mode = *self.fail_mode.lock().await;
        let result = match mode {
            FailMode::Ok => Ok(()),
            FailMode::AlwaysErr => Err(OrchestratorError::Other("mock intervener err".into())),
            FailMode::ErrThenOk(n) => {
                let mut count = self.err_then_ok_count.lock().await;
                *count += 1;
                if *count > n {
                    Ok(())
                } else {
                    Err(OrchestratorError::Other(format!(
                        "mock intervener err {}/{}",
                        *count, n
                    )))
                }
            }
        };
        // 不论成败都记调用——便于断言 retry 次数。
        self.calls
            .lock()
            .await
            .push((agent_id, interrupt, text.to_string()));
        result
    }

    async fn role_of(&self, agent_id: AgentId) -> Option<String> {
        self.roles.lock().await.get(&agent_id).cloned()
    }
}

struct EmptyLookup;
#[async_trait]
impl TriggerLookup for EmptyLookup {
    async fn intent(&self, _id: &str) -> Option<String> {
        None
    }
}

fn empty_lookup() -> Arc<dyn TriggerLookup> {
    Arc::new(EmptyLookup)
}

/// 等 mock 至少被 intervene N 次——避免 sleep-only 等待。
async fn wait_calls(mock: &Arc<MockIntervener>, at_least: usize) {
    for _ in 0..200 {
        if mock.snapshot().await.len() >= at_least {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let got = mock.snapshot().await.len();
    panic!("等 intervene 调用至少 {at_least} 次超时，实际 {got}");
}

// ── e2e 测试 ───────────────────────────────────────────────────

/// 测试 1：中间事件不占玄女 attention，但仍写入 EventStore（公理 2 不破）。
///
/// **白名单 ⇒ 不算"中间事件"，以下事件**绝不**加入此循环**：
/// - `AgentRequestReview` / `ReviewRequestTimeout`（B1 核心，触发 intervene）
/// - `AgentDead` / `TriggerFired` / `OrchestratorCcReceived`（β 保留触发项）
/// - 终态 `TaskStateChanged{to: Done|Cancelled}`（bridge 内部仍触发 worker 报告路径——
///   注意：β #2 完成后默认改回 silent，但安全起见仍排除）
///
/// 推：`AgentResponded` / `ToolCallStarted` / `ToolCallFinished` / 非终态 `TaskStateChanged`。
#[tokio::test]
async fn middle_events_dont_trigger_xuannv_attention() {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let xuannv = AgentId::new();
    let worker = AgentId::new();

    let mock = MockIntervener::new();
    mock.set_role(worker, "luban").await;

    let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
    // 等桥 subscribe ready —— broadcast 漏发给未订阅者。
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 推 N 轮，每轮 4 类中间事件 → 共 20 条。
    let task_id = TaskId::new();
    for i in 0..5 {
        let mut m1 = EventMeta::now();
        m1.agent = Some(worker);
        m1.task = Some(task_id);
        bus.publish(Event {
            meta: m1,
            kind: EventKind::AgentResponded {
                text: format!("step-{i}"),
                artifact_ref: None,
            },
        })
        .expect("publish responded");

        let mut m2 = EventMeta::now();
        m2.agent = Some(worker);
        m2.task = Some(task_id);
        bus.publish(Event {
            meta: m2,
            kind: EventKind::ToolCallStarted {
                tool: "fs.read".into(),
                args: serde_json::json!({"path": format!("file-{i}")}),
            },
        })
        .expect("publish tool start");

        let mut m3 = EventMeta::now();
        m3.agent = Some(worker);
        m3.task = Some(task_id);
        bus.publish(Event {
            meta: m3,
            kind: EventKind::ToolCallFinished {
                tool: "fs.read".into(),
                ok: true,
                output_preview: format!("ok-{i}"),
            },
        })
        .expect("publish tool finish");

        let mut m4 = EventMeta::now();
        m4.agent = Some(worker);
        m4.task = Some(task_id);
        bus.publish(Event {
            meta: m4,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Ready,
                to: fuxi_core::task::TaskState::InProgress,
            },
        })
        .expect("publish task state");
    }

    // 给桥充分消化时间——它没该响应的事件，所以这里只为兜底确认 silent。
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 断言 1：玄女 attention 完全未被占。
    let calls = mock.snapshot().await;
    assert!(
        calls.is_empty(),
        "中间事件不应触发 intervene，实际调用 {} 次：{calls:?}",
        calls.len()
    );

    // 断言 2：所有事件仍写入 SQLite（公理 2 重新定义为"可查"——不可破）。
    // EventStore 写入是异步的，等一下再 replay。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut hist = bus.replay(ReplayCursor::Beginning, false);
    let mut count = 0usize;
    while let Some(item) = hist.next().await {
        if item.is_ok() {
            count += 1;
        }
    }
    // 用 >= 而非 == 留弹性：EventStore 可能有内部元事件。
    assert!(
        count >= 20,
        "EventStore 应至少持久化推入的 20 条中间事件，实际 {count}"
    );
}

/// 测试 2：门客发 `AgentRequestReview` → 桥 intervene 玄女恰好一次，
/// prompt 含 `deliverable_kind` 字面 tag + summary + 门客 role。
#[tokio::test]
async fn deliverable_review_triggers_xuannv_attention() {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let xuannv = AgentId::new();
    let worker = AgentId::new();

    let mock = MockIntervener::new();
    mock.set_role(worker, "luban").await;

    let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
    tokio::time::sleep(Duration::from_millis(20)).await;

    let task_id = TaskId::new();
    let mut meta = EventMeta::now();
    meta.agent = Some(worker);
    meta.task = Some(task_id);
    bus.publish(Event {
        meta,
        kind: EventKind::AgentRequestReview {
            agent: worker,
            task: task_id,
            deliverable_kind: DeliverableKind::ResearchSummary,
            summary: "调研完成: cc/codex 接入路径".into(),
            artifact_ref: Some("docs/research/agent-cli-survey.md".into()),
        },
    })
    .expect("publish review request");

    wait_calls(&mock, 1).await;

    let calls = mock.snapshot().await;
    assert_eq!(
        calls.len(),
        1,
        "AgentRequestReview 应触发恰好 1 次 intervene"
    );
    let (target, _interrupt, prompt) = &calls[0];
    assert_eq!(*target, xuannv, "intervene 目标应为玄女");
    // bridge.rs::build_request_review_prompt 用字面 snake_case tag。
    assert!(
        prompt.contains("deliverable_kind=research_summary"),
        "prompt 应含 deliverable_kind=research_summary: {prompt}"
    );
    assert!(
        prompt.contains("调研完成: cc/codex 接入路径"),
        "prompt 应含 summary 原文: {prompt}"
    );
    assert!(
        prompt.contains("luban"),
        "prompt 应含门客 role（luban）: {prompt}"
    );
    assert!(
        prompt.contains("docs/research/agent-cli-survey.md"),
        "prompt 应含 artifact_ref: {prompt}"
    );
}

/// 测试 3：mock 玄女永远 Err → bridge retry 全失败 → publish `ReviewRequestTimeout`，
/// `original_event_id` 关联回原 `AgentRequestReview` 的 meta.id。
///
/// **走生产 spawn_with 路径**（不用 β 内部的 `spawn_with_backoff_for_test`——那是
/// `pub(crate)`，外部 tests/ 不可见；`REVIEW_RETRY_BACKOFF_MS` 同样 `pub(crate)`）。
/// 生产 backoff = `[200, 500, 1000]` ms，sum = 1700ms。给 3s 预算留余量足够。
///
/// 期望 mock 调用计数（与 β `review_intervene_timeout_publishes_fallback_event` 同源）：
/// - 4 × `[REVIEW_REQUEST]`（首次 1 + retry 3）
/// - ≥1 × `[REVIEW_TIMEOUT]`（兜底事件被白名单触发，且兜底自身也是 Err 但桥不再嵌套 retry）
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_timeout_falls_back_to_event() {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let xuannv = AgentId::new();
    let worker = AgentId::new();

    let mock = MockIntervener::new();
    mock.set_role(worker, "luban").await;
    mock.set_fail(FailMode::AlwaysErr).await;

    // 在 spawn 桥前订阅，避免 broadcast 漏发。
    let mut observer = bus.subscribe();

    let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
    tokio::time::sleep(Duration::from_millis(20)).await;

    let task_id = TaskId::new();
    let mut meta = EventMeta::now();
    meta.agent = Some(worker);
    meta.task = Some(task_id);
    let original_event_id = meta.id;
    bus.publish(Event {
        meta,
        kind: EventKind::AgentRequestReview {
            agent: worker,
            task: task_id,
            deliverable_kind: DeliverableKind::CodeChange,
            summary: "改 X bug，回归测试已绿".into(),
            artifact_ref: Some("commit:abcdef0".into()),
        },
    })
    .expect("publish AgentRequestReview");

    // backoff sum = 1700ms；给 3s 总预算覆盖调度噪音。
    let mut saw_timeout: Option<(uuid::Uuid, AgentId, TaskId, u64)> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(3000);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, observer.next()).await {
            Ok(Some(Ok(ev))) => {
                if let EventKind::ReviewRequestTimeout {
                    original_event_id: oid,
                    agent,
                    task,
                    waited_for_ms,
                } = &ev.kind
                {
                    saw_timeout = Some((*oid, *agent, *task, *waited_for_ms));
                    break;
                }
            }
            Ok(Some(Err(_))) => continue, // Lagged 不致命，继续读
            Ok(None) => break,            // 流结束
            Err(_) => break,              // 总预算超
        }
    }

    let (oid, agent, task, waited_for_ms) =
        saw_timeout.expect("retry 全 fail 后应 publish ReviewRequestTimeout");
    assert_eq!(
        oid, original_event_id,
        "original_event_id 应指向原 AgentRequestReview 的 meta.id"
    );
    assert_eq!(agent, worker, "ReviewRequestTimeout.agent 应保留原 worker");
    assert_eq!(task, task_id, "ReviewRequestTimeout.task 应保留原 task");
    assert!(
        waited_for_ms > 0,
        "waited_for_ms 应反映 backoff 累计，实际 {waited_for_ms}"
    );

    // 兜底：mock 调用次数验证 retry 序列完整跑过。
    // 生产 REVIEW_RETRY_BACKOFF_MS.len() = 3 → 首次 1 + retry 3 = 4 次 REVIEW_REQUEST。
    // 兜底 ReviewRequestTimeout 触发 ≥1 次 REVIEW_TIMEOUT（按白名单）。
    //
    // ReviewRequestTimeout 的 intervene 在桥另一个 loop iteration 里跑，与本测试观察
    // 到事件之间存在调度竞速；wait_calls 直到看到 5 次（4 REVIEW_REQUEST + 1 REVIEW_TIMEOUT）
    // 或超时 panic。
    wait_calls(&mock, 5).await;
    let calls = mock.snapshot().await;
    let review_req = calls
        .iter()
        .filter(|(_, _, t)| t.contains("[REVIEW_REQUEST]"))
        .count();
    assert_eq!(
        review_req, 4,
        "首发 1 + retry 3 = 4 次 REVIEW_REQUEST，实际 {review_req}：{calls:?}"
    );
    let review_timeout = calls
        .iter()
        .filter(|(_, _, t)| t.contains("[REVIEW_TIMEOUT]"))
        .count();
    assert!(
        review_timeout >= 1,
        "兜底 ReviewRequestTimeout 应触发 ≥1 次 REVIEW_TIMEOUT intervene：{calls:?}"
    );
}
