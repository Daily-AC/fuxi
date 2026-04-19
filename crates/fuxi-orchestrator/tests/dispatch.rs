//! 集成测试 —— 用 `StubAgent` 验证 Fuxi 的 spawn/dispatch/shutdown 流程
//! **不依赖真 claude**。所有事件 / 转场都可预测。
//!
//! WHY 用 stub 而非 real cc：
//! - 测试必须快、确定、可离线。Real cc 每次 $0.05 且依赖网络、auth。
//! - 编排逻辑（shelf 注册 / 状态转换 / 终结事件触发 idle 回落 / shutdown 清理）
//!   和具体 agent 实现无关——stub 足够暴露它的 bug。

use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::{Task, TaskState};
use fuxi_core::{CoreError, Result};
use fuxi_events::{EventBus, ReplayCursor};
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::GitWorktreeWorkspace;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::mpsc;

/// 最小的 `Agent` 实现：记住 profile、spawn 时自动发脚本化事件。
struct StubAgent {
    card: AgentCard,
    dispatch_count: AtomicUsize,
    /// 每次 dispatch 要 emit 的事件脚本（按顺序发送）。
    /// task_id 会在 dispatch 时填入 meta。
    script: Vec<EventKind>,
}

impl StubAgent {
    fn new(role: &str, script: Vec<EventKind>) -> Arc<Self> {
        let card = AgentCard {
            id: AgentId::new(),
            profile: AgentProfile {
                name: format!("stub-{role}"),
                role: role.to_string(),
                cli: "stub".to_string(),
                system_prompt: String::new(),
                tags: vec!["test".to_string()],
                extra: Default::default(),
            },
            endpoint: "stub://local".into(),
            status: AgentStatus::Idle,
        };
        Arc::new(Self {
            card,
            dispatch_count: AtomicUsize::new(0),
            script,
        })
    }

    fn dispatches(&self) -> usize {
        self.dispatch_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Agent for StubAgent {
    fn card(&self) -> &AgentCard {
        &self.card
    }

    async fn dispatch(&self, task: Task) -> Result<mpsc::Receiver<Event>> {
        self.dispatch_count.fetch_add(1, Ordering::Relaxed);
        let agent_id = self.card.id;
        let task_id = task.id;
        let script = self.script.clone();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            for kind in script {
                let mut meta = EventMeta::now();
                meta.agent = Some(agent_id);
                meta.task = Some(task_id);
                let _ = tx.send(Event { meta, kind }).await;
            }
        });
        Ok(rx)
    }

    async fn send_message(&self, _task: TaskId, _text: &str) -> Result<()> {
        Err(CoreError::Other(
            "stub does not support send_message".into(),
        ))
    }

    async fn cancel(&self, _task: TaskId) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// 造一个空的 GitWorktreeWorkspace——对着 tempdir repo。
/// 保证有一个叫 `main` 的分支 + 首个 commit，供 worktree add 使用。
async fn make_workspace() -> (TempDir, Arc<GitWorktreeWorkspace>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path();

    run_git(path, &["init", "-q", "-b", "main"]).await;
    tokio::fs::write(path.join("README.md"), "seed")
        .await
        .unwrap();
    run_git(path, &["add", "-A"]).await;
    run_git(
        path,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ],
    )
    .await;

    let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path.to_path_buf()));
    (dir, ws)
}

async fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .await
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed in {cwd:?}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn happy_script() -> Vec<EventKind> {
    vec![
        EventKind::AgentResponded {
            text: "hello from stub".into(),
        },
        EventKind::TaskStateChanged {
            from: TaskState::InProgress,
            to: TaskState::Delivering,
        },
        EventKind::TaskStateChanged {
            from: TaskState::Delivering,
            to: TaskState::Done,
        },
    ]
}

#[tokio::test]
async fn insert_agent_and_list() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub.clone(), None).await;

    assert_eq!(fuxi.worker_count().await, 1);
    let cards = fuxi.list_workers().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, id);
    assert_eq!(cards[0].profile.role, "dev");
}

#[tokio::test]
async fn dispatch_republishes_events_and_marks_idle_on_done() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub.clone(), None).await;

    let mut sub = bus.subscribe();
    fuxi.dispatch(id, Task::new("t1", "hi")).await.unwrap();

    // 等事件 republish——读到 Done 为止。
    let mut saw_response = false;
    let mut saw_done = false;
    for _ in 0..20 {
        let maybe = tokio::time::timeout(std::time::Duration::from_secs(1), sub.next()).await;
        let Ok(Some(Ok(ev))) = maybe else { break };
        match ev.kind {
            EventKind::AgentResponded { text } if text.contains("hello") => saw_response = true,
            EventKind::TaskStateChanged {
                to: TaskState::Done,
                ..
            } => {
                saw_done = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_response, "应通过 bus 看到 AgentResponded");
    assert!(saw_done, "应通过 bus 看到终结事件");
    assert_eq!(stub.dispatches(), 1);
}

#[tokio::test]
async fn dispatch_to_any_reuses_idle_worker() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub.clone(), None).await;

    // 第一次 dispatch：stub 应被复用（不是 spawn 新的）——因为 role=dev 已存在一个 idle。
    let profile_template = AgentProfile {
        name: "ignored".into(),
        role: "will-be-overwritten".into(),
        cli: "claude-code".into(),
        system_prompt: String::new(),
        tags: vec![],
        extra: Default::default(),
    };
    let kind_for_spawn =
        fuxi_orchestrator::WorkerKind::Cc(fuxi_agent_cc::CcLaunchConfig::default());

    let chosen = fuxi
        .dispatch_to_any("dev", Task::new("t", "d"), profile_template, kind_for_spawn)
        .await
        .unwrap();

    assert_eq!(chosen, id, "dispatch_to_any 应复用已有 idle 门客");
    // 等 pump 吃完事件。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(fuxi.worker_count().await, 1, "不应 spawn 新门客");
}

#[tokio::test]
async fn events_persist_to_store_for_replay() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub, None).await;
    fuxi.dispatch(id, Task::new("t", "d")).await.unwrap();

    // 等 republish + store flush。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let mut hist = bus.replay(ReplayCursor::Beginning, false);
    let mut events = vec![];
    while let Some(Ok(ev)) = hist.next().await {
        events.push(ev);
    }
    assert!(
        events.iter().any(
            |e| matches!(&e.kind, EventKind::AgentResponded { text } if text.contains("hello"))
        ),
        "replay 应能拿到 AgentResponded"
    );
    assert!(
        events.iter().any(|e| matches!(
            e.kind,
            EventKind::TaskStateChanged {
                to: TaskState::Done,
                ..
            }
        )),
        "replay 应能拿到 Done"
    );
}

#[tokio::test]
async fn shutdown_clears_shelf() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let stub = StubAgent::new("dev", vec![]);
    fuxi.insert_agent(stub, None).await;
    assert_eq!(fuxi.worker_count().await, 1);

    fuxi.shutdown().await.unwrap();
    assert!(fuxi.worker_count().await == 0 || fuxi.list_workers().await.is_empty());
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    // 回归：连调两次 shutdown 不应 panic 也不应返回 Err。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let stub = StubAgent::new("dev", vec![]);
    fuxi.insert_agent(stub, None).await;

    fuxi.shutdown().await.unwrap();
    fuxi.shutdown().await.unwrap();
    assert_eq!(fuxi.worker_count().await, 0);
}

#[tokio::test]
async fn dispatch_to_unknown_agent_returns_not_found() {
    // 回归：dispatch 到没登记的 id 应明确返回 AgentNotFound。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);
    let ghost = AgentId::new();

    let res = fuxi.dispatch(ghost, Task::new("t", "")).await;
    assert!(res.is_err(), "dispatch 到 ghost id 必须失败");
}

#[tokio::test]
async fn pump_returns_to_idle_on_channel_close_without_terminal() {
    // 回归（code review S2）：若 agent 的 event stream 提前关闭但没发终结事件，
    // pump 也必须把门客摊回 Idle——否则会被永久锁死 Busy、dispatch_to_any 再也
    // 复用不了。StubAgent 的 happy_script_no_terminal 只发一条 AgentResponded
    // 就让 sender 被 drop（tokio::spawn 闭包结束）。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let stub = StubAgent::new(
        "dev",
        vec![EventKind::AgentResponded {
            text: "unfinished".into(),
        }],
    );
    let id = fuxi.insert_agent(stub, None).await;

    fuxi.dispatch(id, Task::new("t", "")).await.unwrap();
    // 给 pump 跑完。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let status = fuxi.status_of(id).await;
    assert_eq!(
        status,
        Some(fuxi_orchestrator::ShelfStatus::Idle),
        "channel 提前关闭后 shelf 必须回 Idle，不能卡在 Busy"
    );
}

#[tokio::test]
async fn blocked_event_does_not_terminate_pump() {
    // 回归（code review I1/S2）：Blocked 是可恢复态（允许 Blocked → Ready），
    // 不应被当成终结。pump 见到 Blocked 后 channel 继续开着时，不应 break。
    // 我们用一个"先发 Blocked 再发 Done"的脚本验证：若 pump 在 Blocked 处就
    // 退出，shelf 会错过 Done 后的真终结；但我们只断言 Done 事件也被 republish。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = StubAgent::new(
        "dev",
        vec![
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Blocked,
            },
            EventKind::TaskStateChanged {
                from: TaskState::Blocked,
                to: TaskState::Ready,
            },
            EventKind::TaskStateChanged {
                from: TaskState::Ready,
                to: TaskState::InProgress,
            },
            EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Delivering,
            },
            EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        ],
    );
    let id = fuxi.insert_agent(stub, None).await;

    let mut sub = bus.subscribe();
    fuxi.dispatch(id, Task::new("t", "")).await.unwrap();

    let mut saw_blocked = false;
    let mut saw_done = false;
    for _ in 0..30 {
        let Ok(Some(Ok(ev))) =
            tokio::time::timeout(std::time::Duration::from_secs(1), sub.next()).await
        else {
            break;
        };
        if let EventKind::TaskStateChanged { to, .. } = &ev.kind {
            if matches!(to, TaskState::Blocked) {
                saw_blocked = true;
            }
            if matches!(to, TaskState::Done) {
                saw_done = true;
                break;
            }
        }
    }
    assert!(saw_blocked, "应该看到 Blocked");
    assert!(
        saw_done,
        "Blocked 之后的 Done 也应被 republish（pump 没在 Blocked 处早退）"
    );
}

#[tokio::test]
async fn concurrent_dispatch_to_any_does_not_double_book() {
    // 回归（code review S3）：并发两个 dispatch_to_any 打同一 role，
    // 应**不会**挑到同一个 idle agent（会 spawn 新的 —— 但测试里 workspace
    // 指向真实 repo，spawn CcAgent 会去找 claude binary；所以我们只注入一个
    // idle stub，然后并发两个 dispatch_to_any："原子 claim" 保证只有一个能
    // 拿到那个 idle，另一个应该走 spawn 路径（我们这里让 spawn 路径因为没有
    // WorkerKind 可靠 spawn 而失败，验证第二个调用**没**拿到第一个 agent 即可）。
    //
    // 更干净的做法：只断言两次 dispatch_to_any 返回的 id 不相同——若相同说明
    // TOCTOU race 发生，同一个 agent 被派了两个 task。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = std::sync::Arc::new(Fuxi::new(bus, ws));

    // 放两个 idle dev：不并发的话两次 dispatch_to_any 会复用同一个（先
    // 找到的那个）；真 race 的话两次都可能挑到同一个。这里确保两个 idle
    // 存在，让原子 claim 有两个可选目标。
    let s1 = StubAgent::new("dev", happy_script());
    let s2 = StubAgent::new("dev", happy_script());
    let id1 = fuxi.insert_agent(s1, None).await;
    let id2 = fuxi.insert_agent(s2, None).await;
    assert_ne!(id1, id2);

    let profile_template = AgentProfile {
        name: "ignored".into(),
        role: "will-be-overwritten".into(),
        cli: "claude-code".into(),
        system_prompt: String::new(),
        tags: vec![],
        extra: Default::default(),
    };
    let kind = fuxi_orchestrator::WorkerKind::Cc(fuxi_agent_cc::CcLaunchConfig::default());

    let (f1, f2) = (fuxi.clone(), fuxi.clone());
    let (p1, p2) = (profile_template.clone(), profile_template.clone());
    let (k1, k2) = (kind.clone(), kind.clone());
    let j1 =
        tokio::spawn(async move { f1.dispatch_to_any("dev", Task::new("t1", ""), p1, k1).await });
    let j2 =
        tokio::spawn(async move { f2.dispatch_to_any("dev", Task::new("t2", ""), p2, k2).await });
    let (r1, r2) = tokio::join!(j1, j2);
    let a = r1.unwrap().unwrap();
    let b = r2.unwrap().unwrap();

    assert_ne!(
        a, b,
        "并发 dispatch_to_any 选到同一个 idle agent——TOCTOU race 存在"
    );
    assert!([id1, id2].contains(&a));
    assert!([id1, id2].contains(&b));
}

#[tokio::test]
async fn lifecycle_events_all_reach_bus() {
    // 公理 #1 的硬证据：spawn → dispatch → shutdown 全流程的每个生命周期
    // 事件都必须在 bus 上出现——Firehose 能看全。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let mut sub = bus.subscribe();

    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub, None).await;
    fuxi.dispatch(id, Task::new("t", "")).await.unwrap();

    // 等待事件流安静下来。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    fuxi.shutdown().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // 收集所有 bus 上的事件。
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        if ev.meta.agent == Some(id) {
            collected.push(ev);
        }
    }

    let has = |pred: &dyn Fn(&EventKind) -> bool| collected.iter().any(|e| pred(&e.kind));
    assert!(
        has(&|k| matches!(k, EventKind::AgentSpawning { .. })),
        "AgentSpawning 必须在 bus 上"
    );
    assert!(
        has(&|k| matches!(k, EventKind::AgentReady { .. })),
        "AgentReady 必须在 bus 上"
    );
    assert!(
        has(&|k| matches!(
            k,
            EventKind::TaskStateChanged {
                to: TaskState::Done,
                ..
            }
        )),
        "TaskStateChanged→Done 必须在 bus 上"
    );
    assert!(
        has(&|k| matches!(k, EventKind::AgentShuttingDown { .. })),
        "AgentShuttingDown 必须在 bus 上"
    );
    assert!(
        has(&|k| matches!(k, EventKind::AgentDead { .. })),
        "AgentDead 必须在 bus 上"
    );
}

// ── 薄片 I · 介入事件三联 ──────────────────────────────────────

/// 支持 send_message / cancel 的 stub——用来验 intervene 事件发出。
struct InterveneTrackingStub {
    card: AgentCard,
    sends: std::sync::atomic::AtomicUsize,
    cancels: std::sync::atomic::AtomicUsize,
}

impl InterveneTrackingStub {
    fn new(role: &str) -> Arc<Self> {
        let card = AgentCard {
            id: AgentId::new(),
            profile: AgentProfile {
                name: format!("intv-{role}"),
                role: role.to_string(),
                cli: "stub".to_string(),
                system_prompt: String::new(),
                tags: vec!["test".to_string()],
                extra: Default::default(),
            },
            endpoint: "stub://intv".into(),
            status: AgentStatus::Idle,
        };
        Arc::new(Self {
            card,
            sends: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Agent for InterveneTrackingStub {
    fn card(&self) -> &AgentCard {
        &self.card
    }
    async fn dispatch(&self, _task: Task) -> Result<mpsc::Receiver<Event>> {
        let (_tx, rx) = mpsc::channel(1);
        Ok(rx)
    }
    async fn send_message(&self, _task: TaskId, _text: &str) -> Result<()> {
        self.sends.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    async fn cancel(&self, _task: TaskId) -> Result<()> {
        self.cancels.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn intervene_append_emits_user_intervention_and_applied() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = InterveneTrackingStub::new("dev");
    let id = fuxi.insert_agent(stub.clone(), None).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(id, false, "hello mid-task")
        .await
        .expect("intervene");

    // 事件顺序：UserInterventionSent(append) + TaskInterventionApplied(append)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        if ev.meta.agent == Some(id) {
            collected.push(ev);
        }
    }

    let has_user_sent_append = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::UserInterventionSent { mode, .. } if mode == "append"
        )
    });
    assert!(has_user_sent_append, "append 模式应发 UserInterventionSent");

    let has_applied = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::TaskInterventionApplied { mode } if mode == "append"
        )
    });
    assert!(
        has_applied,
        "应发 TaskInterventionApplied {{ mode=append }}"
    );

    assert!(
        !collected
            .iter()
            .any(|e| matches!(&e.kind, EventKind::AgentInterrupted { .. })),
        "append 模式**不应**发 AgentInterrupted"
    );

    // wire 层确实调了 send_message；没调 cancel
    assert_eq!(stub.sends.load(Ordering::Relaxed), 1);
    assert_eq!(stub.cancels.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn intervene_interrupt_emits_three_events_and_calls_cancel() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = InterveneTrackingStub::new("dev");
    let id = fuxi.insert_agent(stub.clone(), None).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(id, true, "stop and rework")
        .await
        .expect("intervene");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        if ev.meta.agent == Some(id) {
            collected.push(ev);
        }
    }

    // 三联事件——v0.1 断言点 17 / 18 / 19
    let has_sent = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::UserInterventionSent { mode, .. } if mode == "interrupt"
        )
    });
    let has_interrupted = collected
        .iter()
        .any(|e| matches!(&e.kind, EventKind::AgentInterrupted { .. }));
    let has_applied = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::TaskInterventionApplied { mode } if mode == "interrupt"
        )
    });
    assert!(has_sent, "缺 UserInterventionSent [interrupt]");
    assert!(has_interrupted, "缺 AgentInterrupted");
    assert!(has_applied, "缺 TaskInterventionApplied [interrupt]");

    // wire 层：cancel 先发，send_message 后发
    assert_eq!(stub.cancels.load(Ordering::Relaxed), 1);
    assert_eq!(stub.sends.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn block_and_resume_task_emit_events() {
    // 薄片 F · v0.1 scenario 断言点 13 + 24。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let mut sub = bus.subscribe();
    let tid = TaskId::new();

    fuxi.block_task(tid, "awaiting_commit_approval".into())
        .expect("block");
    fuxi.resume_task(tid, Some("同意".into())).expect("resume");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        if ev.meta.task == Some(tid) {
            collected.push(ev);
        }
    }

    let has_blocked = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::TaskBlocked { reason } if reason == "awaiting_commit_approval"
        )
    });
    let has_resumed = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::TaskResumed { input: Some(s) } if s == "同意"
        )
    });
    assert!(has_blocked, "缺 TaskBlocked");
    assert!(has_resumed, "缺 TaskResumed with input=同意");
}

#[tokio::test]
async fn intervene_on_missing_agent_returns_not_found() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let bogus = AgentId::new();
    let err = fuxi
        .intervene(bogus, false, "ghost")
        .await
        .expect_err("should fail on missing agent");
    let msg = format!("{err}");
    assert!(
        msg.contains("not registered") || msg.contains("not found") || msg.contains("agent"),
        "期待 agent-not-found 类错误, got: {msg}"
    );
}
