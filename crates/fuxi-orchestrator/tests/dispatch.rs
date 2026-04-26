//! 集成测试 —— 用 `StubAgent` 验证 Fuxi 的 spawn/dispatch/shutdown 流程
//! **不依赖真 claude**。所有事件 / 转场都可预测。
//!
//! WHY 用 stub 而非 real cc：
//! - 测试必须快、确定、可离线。Real cc 每次 $0.05 且依赖网络、auth。
//! - 编排逻辑（shelf 注册 / 状态转换 / 终结事件触发 idle 回落 / shutdown 清理）
//!   和具体 agent 实现无关——stub 足够暴露它的 bug。

use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_agent_codex::CodexLaunchConfig;
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::{Task, TaskState};
use fuxi_core::{CoreError, Result};
use fuxi_events::{EventBus, ReplayCursor};
use fuxi_orchestrator::{Fuxi, FuxiConfig, WorkerKind};
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
    /// P2 召回测试用：override `session_id()` 返回固定值。None = 走默认 None。
    session_id_override: Option<String>,
}

impl StubAgent {
    fn new(role: &str, script: Vec<EventKind>) -> Arc<Self> {
        Self::with_session_id(role, script, None)
    }

    /// P2 召回测试用：构造一个能返回固定 session_id 的 stub。
    fn with_session_id(
        role: &str,
        script: Vec<EventKind>,
        session_id_override: Option<String>,
    ) -> Arc<Self> {
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
            session_id_override,
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

    async fn session_id(&self) -> Option<String> {
        self.session_id_override.clone()
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
async fn dispatch_in_task_can_fan_out_same_parent_task_to_multiple_workers() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let a = StubAgent::new("dev", happy_script());
    let b = StubAgent::new("dev", happy_script());
    let aid = fuxi.insert_agent(a, None).await;
    let bid = fuxi.insert_agent(b, None).await;
    let parent = TaskId::new();

    let mut sub = bus.subscribe();
    fuxi.dispatch_in_task(aid, parent, "修 auth bug", "A")
        .await
        .unwrap();
    fuxi.dispatch_in_task(bid, parent, "修 auth bug", "B")
        .await
        .unwrap();

    let mut seen_dispatch_to_a = false;
    let mut seen_dispatch_to_b = false;
    for _ in 0..40 {
        let Ok(Some(Ok(ev))) =
            tokio::time::timeout(std::time::Duration::from_millis(200), sub.next()).await
        else {
            break;
        };
        if ev.meta.task != Some(parent) {
            continue;
        }
        if let EventKind::TaskDispatched { to } = ev.kind {
            if to == aid {
                seen_dispatch_to_a = true;
            }
            if to == bid {
                seen_dispatch_to_b = true;
            }
            if seen_dispatch_to_a && seen_dispatch_to_b {
                break;
            }
        }
    }
    assert!(seen_dispatch_to_a, "同父任务应派发到门客 A");
    assert!(seen_dispatch_to_b, "同父任务应派发到门客 B");
}

#[tokio::test]
async fn dispatch_to_any_in_task_spawns_new_worker_even_when_idle_exists() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::with_config(
        bus.clone(),
        ws,
        FuxiConfig {
            allocate_worktree: false,
            ..Default::default()
        },
    );

    let idle = StubAgent::new("dev", happy_script());
    let idle_id = fuxi.insert_agent(idle.clone(), None).await;

    let profile_template = AgentProfile {
        name: "spawned".into(),
        role: "placeholder".into(),
        cli: "codex".into(),
        system_prompt: String::new(),
        tags: vec![],
        extra: Default::default(),
    };
    let kind_for_spawn = WorkerKind::Codex(CodexLaunchConfig {
        argv_prefix: vec![],
        binary: "/usr/bin/true".into(),
        model: String::new(),
        cwd: None,
        full_auto: true,
        bypass_approvals: true,
        extra_args: vec![],
    });
    let parent = TaskId::new();

    let mut sub = bus.subscribe();
    let chosen = fuxi
        .dispatch_to_any_in_task(
            "dev",
            parent,
            "修 auth bug",
            "严格 task-bound path",
            profile_template,
            kind_for_spawn,
        )
        .await
        .unwrap();

    assert_ne!(
        chosen, idle_id,
        "严格 task-bound 派工不应复用现有 idle 门客"
    );
    assert_eq!(idle.dispatches(), 0, "现有 idle 不应被派工复用");
    assert_eq!(fuxi.worker_count().await, 2, "应显式 spawn 出第二个门客");

    let mut saw_task_dispatch = false;
    for _ in 0..20 {
        let Ok(Some(Ok(ev))) =
            tokio::time::timeout(std::time::Duration::from_millis(200), sub.next()).await
        else {
            break;
        };
        if let EventKind::TaskDispatched { to } = ev.kind
            && to == chosen
            && ev.meta.task == Some(parent)
        {
            saw_task_dispatch = true;
            break;
        }
    }
    assert!(
        saw_task_dispatch,
        "严格 task-bound 派工应把同一 task_id 绑定到新 spawn 的门客上"
    );
}

#[tokio::test]
async fn dispatch_to_any_is_legacy_shell_and_spawns_task_bound_worker() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::with_config(
        bus.clone(),
        ws,
        FuxiConfig {
            allocate_worktree: false,
            ..Default::default()
        },
    );

    let stub = StubAgent::new("dev", happy_script());
    let idle_id = fuxi.insert_agent(stub.clone(), None).await;

    let profile_template = AgentProfile {
        name: "ignored".into(),
        role: "will-be-overwritten".into(),
        cli: "codex".into(),
        system_prompt: String::new(),
        tags: vec![],
        extra: Default::default(),
    };
    let kind_for_spawn = WorkerKind::Codex(CodexLaunchConfig {
        argv_prefix: vec![],
        binary: "/usr/bin/true".into(),
        model: String::new(),
        cwd: None,
        full_auto: true,
        bypass_approvals: true,
        extra_args: vec![],
    });

    let chosen = fuxi
        .dispatch_to_any("dev", Task::new("t", "d"), profile_template, kind_for_spawn)
        .await
        .unwrap();

    assert_ne!(
        chosen, idle_id,
        "legacy 壳应统一到 task-bound，不再复用 idle"
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(stub.dispatches(), 0, "旧 idle 门客不应被 legacy 壳复用");
    assert_eq!(fuxi.worker_count().await, 2, "应 spawn 新门客并绑定 task");
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

/// #19 回归 · `EventKind::TaskBlocked` 是 cc/codex 的"本 turn 出错"信号
/// （cc `ResultError` / codex `TurnFailed` 都翻译到这里），与状态机的
/// `TaskStateChanged{to: Blocked}` 不同——前者 turn 已结束 cc 内部 Idle，
/// 后者只是状态转移仍可能继续派事件。pump 必须把 `TaskBlocked` 视为终态，
/// 否则 cc 报错后 shelf 锁死 Busy（用户实测撞过：门客 Idle 但任务无收尾）。
#[tokio::test]
async fn task_blocked_terminates_dispatch_pump_so_shelf_returns_idle() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    // 脚本只有一条 `TaskBlocked`——模拟 cc `ResultError`。pump 应当看到此事件后
    // 退出并把 shelf 摊回 Idle，不再等永远不会来的 Done。
    let stub = StubAgent::new(
        "dev",
        vec![EventKind::TaskBlocked {
            reason: "cc cli error".into(),
        }],
    );
    let id = fuxi.insert_agent(stub, None).await;

    fuxi.dispatch(id, Task::new("t", "")).await.unwrap();
    // 给 pump 跑完（含 50ms grace 窗口）。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let status = fuxi.status_of(id).await;
    assert_eq!(
        status,
        Some(fuxi_orchestrator::ShelfStatus::Idle),
        "TaskBlocked 后 shelf 必须回 Idle，否则门客锁死 Busy"
    );
}

#[tokio::test]
async fn concurrent_dispatch_to_any_spawns_distinct_task_bound_workers() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = std::sync::Arc::new(Fuxi::with_config(
        bus,
        ws,
        FuxiConfig {
            allocate_worktree: false,
            ..Default::default()
        },
    ));

    let s1 = StubAgent::new("dev", happy_script());
    let s2 = StubAgent::new("dev", happy_script());
    fuxi.insert_agent(s1, None).await;
    fuxi.insert_agent(s2, None).await;

    let profile_template = AgentProfile {
        name: "ignored".into(),
        role: "will-be-overwritten".into(),
        cli: "codex".into(),
        system_prompt: String::new(),
        tags: vec![],
        extra: Default::default(),
    };
    let kind = WorkerKind::Codex(CodexLaunchConfig {
        argv_prefix: vec![],
        binary: "/usr/bin/true".into(),
        model: String::new(),
        cwd: None,
        full_auto: true,
        bypass_approvals: true,
        extra_args: vec![],
    });

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
        "legacy 壳并发派工应各自 spawn 新门客，不能返回相同 id"
    );
    assert_eq!(fuxi.worker_count().await, 4, "两个旧 idle + 两个新 spawn");
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
    dispatches: std::sync::atomic::AtomicUsize,
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
            dispatches: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Agent for InterveneTrackingStub {
    fn card(&self) -> &AgentCard {
        &self.card
    }
    async fn dispatch(&self, _task: Task) -> Result<mpsc::Receiver<Event>> {
        self.dispatches.fetch_add(1, Ordering::Relaxed);
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
    // 2026-04-20 bug 修复后 intervene 对 Idle 门客自动退化 dispatch；
    // 本用例检的是 Busy 下 append 行为，显式先把 shelf status 置 Busy。
    use fuxi_orchestrator::ShelfStatus;
    fuxi.clone_shelf().set_status(id, ShelfStatus::Busy).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(id, false, "hello mid-task", Vec::new())
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
    // 打断语义只对 Busy 门客有意义——显式置 Busy。
    use fuxi_orchestrator::ShelfStatus;
    fuxi.clone_shelf().set_status(id, ShelfStatus::Busy).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(id, true, "stop and rework", Vec::new())
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

/// 2026-04-20 用户 + 玄女实测发现：spawn 门客后 intervene 不走 dispatch →
/// cc active_tx=None → 响应被 drop。修复：intervene 对 Idle 门客自动退化 dispatch。
#[tokio::test]
async fn intervene_on_idle_auto_degrades_to_dispatch() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = InterveneTrackingStub::new("dev");
    let id = fuxi.insert_agent(stub.clone(), None).await;
    // shelf 新 insert 的 agent 默认 Idle —— 正是我们要验证的场景。
    use fuxi_orchestrator::ShelfStatus;
    assert_eq!(
        fuxi.clone_shelf().status_of(id).await,
        Some(ShelfStatus::Idle)
    );

    let mut sub = bus.subscribe();
    fuxi.intervene(id, false, "你好，鲁班", Vec::new())
        .await
        .expect("intervene");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        if ev.meta.agent == Some(id) {
            collected.push(ev);
        }
    }

    // 应发 UserInterventionSent 且 mode="append_via_dispatch"（退化标记）
    let has_via_dispatch = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::UserInterventionSent { mode, .. } if mode == "append_via_dispatch"
        )
    });
    assert!(
        has_via_dispatch,
        "Idle intervene 应发 UserInterventionSent {{ mode=append_via_dispatch }}"
    );

    // 2026-04-20 改：degrade 出来的 task title 应为 "user-turn"——语义和 TUI
    // Submit::Xuannv 的 user 对话 task 统一，避免 TUI 同时看到两种名字的 task。
    let has_user_turn_title = collected.iter().any(|e| {
        matches!(
            &e.kind,
            EventKind::TaskCreated { title, .. } if title == "user-turn"
        )
    });
    assert!(
        has_user_turn_title,
        "Idle intervene degrade 的 task title 应为 user-turn，collected={:?}",
        collected
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::TaskCreated { title, .. } => Some(title.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    );

    // wire 层：走 dispatch 路径 —— stub.dispatches 应为 1，send_message 为 0
    assert_eq!(
        stub.dispatches.load(Ordering::Relaxed),
        1,
        "应走 dispatch 路径"
    );
    assert_eq!(
        stub.sends.load(Ordering::Relaxed),
        0,
        "不应再走 send_message（会被 drop）"
    );

    // shelf 被 dispatch 置 Busy（dispatch pump 还没看到 terminal 所以不会反弹回 Idle）
    // 这里只验退化语义，不验状态转回
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
        .intervene(bogus, false, "ghost", Vec::new())
        .await
        .expect_err("should fail on missing agent");
    let msg = format!("{err}");
    assert!(
        msg.contains("not registered") || msg.contains("not found") || msg.contains("agent"),
        "期待 agent-not-found 类错误, got: {msg}"
    );
}

// ── M1.5 · orchestrator 补课 ──────────────────────────────────────

#[tokio::test]
async fn shelf_worktree_of_returns_path_when_allocated() {
    use fuxi_core::workspace::WorkspaceHandle;
    use fuxi_orchestrator::{Shelf, ShelfEntry, ShelfStatus};
    use std::path::PathBuf;

    let shelf = Shelf::new();
    let stub = InterveneTrackingStub::new("dev");
    let id = stub.card.id;
    let wt = WorkspaceHandle {
        agent: id,
        repo_root: PathBuf::from("/tmp/fuxi-repo"),
        worktree_path: PathBuf::from("/tmp/fuxi-wt/agent-1"),
        branch: "main".into(),
        borrowed: false,
    };
    shelf
        .insert(ShelfEntry {
            card: stub.card.clone(),
            agent: stub.clone() as Arc<dyn Agent>,
            status: ShelfStatus::Idle,
            worktree: Some(wt.clone()),
            idle_since: Some(std::time::Instant::now()),
        })
        .await;

    let got = shelf.worktree_of(id).await;
    assert_eq!(got, Some(wt.worktree_path.clone()));

    // 未登记的 id 返回 None
    let ghost = AgentId::new();
    assert!(shelf.worktree_of(ghost).await.is_none());
}

#[tokio::test]
async fn intervene_on_worker_publishes_cc_received_to_xuannv() {
    // 抄送：用户对门客说话，玄女同时收到副本。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let xuannv = InterveneTrackingStub::new("xuannv");
    let worker = InterveneTrackingStub::new("dev");
    let xuannv_id = fuxi.insert_agent(xuannv, None).await;
    let worker_id = fuxi.insert_agent(worker, None).await;
    fuxi.set_xuannv(xuannv_id).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(worker_id, false, "加个注释", Vec::new())
        .await
        .expect("intervene worker");

    // 收集事件
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        collected.push(ev);
    }

    // 应有一条 OrchestratorCcReceived，meta.agent = xuannv，text 匹配
    let cc = collected.iter().find(|e| {
        matches!(
            &e.kind,
            EventKind::OrchestratorCcReceived { from_user_to, text, .. }
                if *from_user_to == worker_id && text == "加个注释"
        )
    });
    assert!(
        cc.is_some(),
        "应发 OrchestratorCcReceived，collected: {collected:#?}"
    );
    assert_eq!(
        cc.unwrap().meta.agent,
        Some(xuannv_id),
        "抄送事件 meta.agent 应为玄女"
    );
}

#[tokio::test]
async fn intervene_on_xuannv_herself_does_not_publish_cc_received() {
    // 玄女收自己的不算抄送——这是噪音。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let xuannv = InterveneTrackingStub::new("xuannv");
    let xuannv_id = fuxi.insert_agent(xuannv, None).await;
    fuxi.set_xuannv(xuannv_id).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(xuannv_id, false, "hi", Vec::new())
        .await
        .expect("intervene xuannv");

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        collected.push(ev);
    }

    assert!(
        !collected
            .iter()
            .any(|e| matches!(&e.kind, EventKind::OrchestratorCcReceived { .. })),
        "对玄女自身的 intervene 不应产生抄送"
    );
}

#[tokio::test]
async fn intervene_without_xuannv_id_set_skips_cc_received() {
    // 玄女 id 还没告知 Fuxi 时，抄送路径不该崩。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let worker = InterveneTrackingStub::new("dev");
    let worker_id = fuxi.insert_agent(worker, None).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(worker_id, false, "hi", Vec::new())
        .await
        .expect("intervene");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut collected = vec![];
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        collected.push(ev);
    }

    assert!(
        !collected
            .iter()
            .any(|e| matches!(&e.kind, EventKind::OrchestratorCcReceived { .. })),
        "未设 xuannv 时不应产生抄送"
    );
}

#[tokio::test]
async fn agent_dead_event_flips_shelf_status_to_dead() {
    // Fuxi 自订阅 bus：看到 AgentDead 即把对应 shelf 条目置 Dead。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = InterveneTrackingStub::new("dev");
    let id = fuxi.insert_agent(stub, None).await;
    assert_eq!(
        fuxi.status_of(id).await,
        Some(fuxi_orchestrator::ShelfStatus::Idle)
    );

    // 直接向 bus 发一条 AgentDead——模拟 CcAgent/外部侦测到死亡
    let mut meta = fuxi_core::event::EventMeta::now();
    meta.agent = Some(id);
    bus.publish(Event {
        meta,
        kind: EventKind::AgentDead {
            cause: "ws closed".into(),
        },
    })
    .expect("publish");

    // Fuxi 的自订阅 task 应在短时内翻状态
    for _ in 0..20 {
        if fuxi.status_of(id).await == Some(fuxi_orchestrator::ShelfStatus::Dead) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("AgentDead 后 shelf 未在 400ms 内翻 Dead");
}

#[tokio::test]
async fn orchestrator_cc_received_carries_original_intervention_id() {
    // OrchestratorCcReceived 应携带 original_intervention_id 字段，方便 TUI 关联到源事件。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let xuannv = InterveneTrackingStub::new("xuannv");
    let worker = InterveneTrackingStub::new("dev");
    let xuannv_id = fuxi.insert_agent(xuannv, None).await;
    let worker_id = fuxi.insert_agent(worker, None).await;
    fuxi.set_xuannv(xuannv_id).await;

    let mut sub = bus.subscribe();
    fuxi.intervene(worker_id, false, "ping", Vec::new())
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let mut sent_id: Option<uuid::Uuid> = None;
    let mut cc_orig: Option<uuid::Uuid> = None;
    while let Ok(Some(Ok(ev))) =
        tokio::time::timeout(std::time::Duration::from_millis(50), sub.next()).await
    {
        match &ev.kind {
            EventKind::UserInterventionSent { target, .. } if *target == worker_id => {
                sent_id = Some(ev.meta.id);
            }
            EventKind::OrchestratorCcReceived {
                original_intervention_id,
                ..
            } => {
                cc_orig = Some(*original_intervention_id);
            }
            _ => {}
        }
    }
    let sent = sent_id.expect("UserInterventionSent 缺失");
    let orig = cc_orig.expect("OrchestratorCcReceived 缺失");
    assert_eq!(
        orig, sent,
        "OrchestratorCcReceived.original_intervention_id 应等于 UserInterventionSent 的事件 id"
    );
}

/// 玄女豁免（2026-04-20 用户质疑）：`shutdown_agent` 不能杀玄女本人——
/// 她是用户对话唯一入口，被 kill 整个 TUI 崩。GC / 将来的 `fuxi kill --id`
/// 都走这个豁免，只有 `Fuxi::shutdown()` 能关玄女。
#[tokio::test]
async fn shutdown_agent_refuses_to_kill_xuannv() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let xuannv = StubAgent::new("xuannv", happy_script());
    let xuannv_id = fuxi.insert_agent(xuannv, None).await;
    fuxi.set_xuannv(xuannv_id).await;

    // 走 shutdown_agent 试杀——应静默 noop（Ok），玄女仍在 shelf
    fuxi.shutdown_agent(xuannv_id, "idle_ttl".into())
        .await
        .expect("shutdown_agent 对玄女应返 Ok 不报错");
    let cards = fuxi.list_workers().await;
    assert!(
        cards.iter().any(|c| c.id == xuannv_id),
        "shutdown_agent 被豁免后玄女应仍在 shelf"
    );

    // 普通门客照常能被 shutdown
    let worker = StubAgent::new("dev", happy_script());
    let worker_id = fuxi.insert_agent(worker, None).await;
    fuxi.shutdown_agent(worker_id, "idle_ttl".into())
        .await
        .expect("普通门客可以被 shutdown");
    let cards = fuxi.list_workers().await;
    assert!(
        !cards.iter().any(|c| c.id == worker_id),
        "普通门客被 shutdown_agent 清走"
    );
    assert!(cards.iter().any(|c| c.id == xuannv_id), "玄女仍在");
}

/// Bug 修复（2026-04-20 用户复测）：`Fuxi::dispatch` 必须在开头发
/// `TaskCreated` + `TaskDispatched` 两条事件，否则 TUI 左栏的"空闲门客"
/// 桶永远不会把被派活的门客移走——`upsert_task` 没收到事件就不会触发。
#[tokio::test]
async fn dispatch_publishes_task_created_and_dispatched_at_start() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub.clone(), None).await;

    let mut sub = bus.subscribe();
    fuxi.dispatch(id, Task::new("scout", "调研一下仓库"))
        .await
        .unwrap();

    // 读前几条事件，至少看到 TaskCreated + TaskDispatched 各一次。
    let mut saw_created = false;
    let mut saw_dispatched = false;
    for _ in 0..10 {
        let maybe = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await;
        let Ok(Some(Ok(ev))) = maybe else { break };
        match &ev.kind {
            EventKind::TaskCreated { title, .. } if title == "scout" => {
                assert_eq!(ev.meta.agent, Some(id), "TaskCreated 应挂 agent meta");
                assert!(ev.meta.task.is_some(), "TaskCreated 应挂 task meta");
                saw_created = true;
            }
            EventKind::TaskDispatched { to } => {
                assert_eq!(*to, id, "TaskDispatched.to 应等于派活目标");
                assert_eq!(ev.meta.agent, Some(id));
                assert!(ev.meta.task.is_some());
                saw_dispatched = true;
            }
            _ => {}
        }
        if saw_created && saw_dispatched {
            break;
        }
    }
    assert!(saw_created, "dispatch 开头应发 TaskCreated");
    assert!(saw_dispatched, "dispatch 开头应发 TaskDispatched");
}

// ── P2 召回 · RecallSink 入库 task→session 映射（dispatch pump 钩子）──

/// 测试用 RecallSink：记录所有 record 调用便于断言。
#[derive(Default, Clone)]
struct CapturingRecallSink {
    /// 收到的完整 RecallContext。clone 共享让 worker / 主线都能读。
    captured: Arc<std::sync::Mutex<Vec<fuxi_orchestrator::RecallContext>>>,
}

impl CapturingRecallSink {
    fn new() -> Self {
        Self::default()
    }

    fn snapshot(&self) -> Vec<fuxi_orchestrator::RecallContext> {
        self.captured.lock().expect("lock").clone()
    }
}

#[async_trait]
impl fuxi_orchestrator::RecallSink for CapturingRecallSink {
    async fn record(&self, ctx: fuxi_orchestrator::RecallContext) {
        self.captured.lock().expect("lock").push(ctx);
    }
}

#[tokio::test]
async fn dispatch_pump_records_session_on_done() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let sink = Arc::new(CapturingRecallSink::new());
    fuxi.set_recall_sink(sink.clone()).await;

    let stub = StubAgent::with_session_id("dev", happy_script(), Some("sess-stub-123".into()));
    let id = fuxi.insert_agent(stub.clone(), None).await;

    let task = Task::new("recall-target", "");
    let task_id = task.id;
    fuxi.dispatch(id, task).await.unwrap();

    // 等 pump 处理完 Done + 调 sink。
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let recorded = sink.snapshot();
    assert_eq!(recorded.len(), 1, "Done 时应入库一次，实际 {recorded:?}");
    let ctx = &recorded[0];
    assert_eq!(ctx.agent_id, id, "agent_id 错");
    assert_eq!(ctx.task_id, task_id, "task_id 错");
    assert_eq!(ctx.cli_session_id.as_deref(), Some("sess-stub-123"));
    assert_eq!(ctx.role, "dev", "role 应来自 agent.card.profile");
    // StubAgent 是 insert_agent 不分 worktree → None。L2 设计：worktree 缺失也调 sink。
    assert!(ctx.worktree.is_none(), "stub agent 不该有 worktree");
}

#[tokio::test]
async fn dispatch_pump_skips_record_on_cancelled() {
    // 设计：召回基于完成态。Cancelled 不入库——避免脏数据被 `--recall` 拉出。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let sink = Arc::new(CapturingRecallSink::new());
    fuxi.set_recall_sink(sink.clone()).await;

    let cancelled_script = vec![EventKind::TaskStateChanged {
        from: TaskState::InProgress,
        to: TaskState::Cancelled,
    }];
    let stub = StubAgent::with_session_id("dev", cancelled_script, Some("sess-cancelled".into()));
    let id = fuxi.insert_agent(stub, None).await;

    fuxi.dispatch(id, Task::new("doomed", "")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    assert!(
        sink.snapshot().is_empty(),
        "Cancelled 不应入库，实际 {:?}",
        sink.snapshot()
    );
}

#[tokio::test]
async fn dispatch_pump_records_even_when_session_id_none() {
    // L2 设计变化：pump 不再以 cli_session_id 守门。codex 这类无 session 的门客
    // 仍要进 sink（worktree 复用走得通）。sink 自行决定哪些字段值得落 fact。
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus, ws);

    let sink = Arc::new(CapturingRecallSink::new());
    fuxi.set_recall_sink(sink.clone()).await;

    // 默认 session_id_override=None
    let stub = StubAgent::new("dev", happy_script());
    let id = fuxi.insert_agent(stub, None).await;

    fuxi.dispatch(id, Task::new("no-sess", "")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let recorded = sink.snapshot();
    assert_eq!(recorded.len(), 1, "Done 仍应触发 sink");
    assert_eq!(recorded[0].agent_id, id);
    assert!(
        recorded[0].cli_session_id.is_none(),
        "cli_session_id 应继承 agent.session_id() = None"
    );
}

/// #7 公理 #3：`xuannv_id_watch` 真实时入口——set_xuannv 触发 changed()，
/// 替代旧 5min/2s 轮询。
#[tokio::test]
async fn xuannv_id_watch_signals_change_when_set() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));

    let mut rx = fuxi.xuannv_id_watch();
    // 初值 None——未设置
    assert!(rx.borrow_and_update().is_none());

    let fuxi_clone = fuxi.clone();
    let new_id = AgentId::new();
    let setter = tokio::spawn(async move {
        // 给订阅方一点时间挂上去（虽然 watch 即使 set 在前也能记到 mark_changed）
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        fuxi_clone.set_xuannv(new_id).await;
    });

    // 真实时唤醒——不轮询，直接 await changed()
    let changed = tokio::time::timeout(std::time::Duration::from_secs(2), rx.changed())
        .await
        .expect("watch changed() 应在 2s 内被 set_xuannv 唤醒");
    assert!(changed.is_ok());
    assert_eq!(*rx.borrow_and_update(), Some(new_id));

    setter.await.unwrap();
}

/// #7：`xuannv_id_watch` 已就绪场景——subscribe 之前就已 set 的话，第一次
/// `borrow` 立即拿到值，不必 .changed() 也能直接走。
#[tokio::test]
async fn xuannv_id_watch_borrow_returns_already_set_value() {
    let bus = EventBus::with_memory_store().await.unwrap();
    let (_dir, ws) = make_workspace().await;
    let fuxi = Arc::new(Fuxi::new(bus, ws));

    let preset = AgentId::new();
    fuxi.set_xuannv(preset).await;
    let mut rx = fuxi.xuannv_id_watch();

    assert_eq!(*rx.borrow_and_update(), Some(preset));
}
