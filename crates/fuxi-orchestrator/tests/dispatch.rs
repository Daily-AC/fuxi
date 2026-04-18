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
