//! Extractor（M2.5）行为契约测试。
//!
//! 约束：fuxi-memory 不依赖 fuxi-orchestrator，所以这里所有测试都只走
//! `EventBus` + 假的 `FactExtractorSpawner`（mock trait 实现），不去碰真正的
//! cc headless。gated E2E 在另一个测试里。

use async_trait::async_trait;
use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskId, TaskState};
use fuxi_events::EventBus;
use fuxi_memory::{Extractor, ExtractorConfig, FactExtractorSpawner, OracleStore, SpawnerResult};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

/// Mock spawner：把调用到的 prompt / agent_id 记录下来，按预制脚本返回文本。
///
/// 只做一件事——配合 Extractor 的单元契约测试；不在这里 mock A2A / cc / WS。
struct MockSpawner {
    calls: Mutex<Vec<String>>,
    roles: Mutex<std::collections::HashMap<AgentId, String>>,
    script: Mutex<Vec<SpawnerResult<String>>>,
    call_count: AtomicUsize,
}

impl MockSpawner {
    fn with_script(script: Vec<SpawnerResult<String>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            roles: Mutex::new(std::collections::HashMap::new()),
            script: Mutex::new(script),
            call_count: AtomicUsize::new(0),
        })
    }

    async fn set_role(&self, agent: AgentId, role: &str) {
        self.roles.lock().await.insert(agent, role.to_string());
    }

    async fn prompts(&self) -> Vec<String> {
        self.calls.lock().await.clone()
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl FactExtractorSpawner for MockSpawner {
    async fn spawn_and_run(&self, prompt: String, _timeout: Duration) -> SpawnerResult<String> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().await.push(prompt);
        let mut script = self.script.lock().await;
        if script.is_empty() {
            return Ok(String::from("[]"));
        }
        script.remove(0)
    }

    async fn role_of(&self, agent: AgentId) -> Option<String> {
        self.roles.lock().await.get(&agent).cloned()
    }
}

async fn bus_and_oracle() -> (EventBus, Arc<OracleStore>) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let oracle = Arc::new(OracleStore::connect_memory().await.expect("oracle"));
    (bus, oracle)
}

/// 发几条构成 task transcript 的事件 + 最终的 Done 状态变更。
/// `agent` 为 `None` 时 meta.agent 留空——Extractor 该按 fallback 逻辑处理。
async fn publish_task_transcript(
    bus: &EventBus,
    task: TaskId,
    agent: Option<AgentId>,
    user: &str,
    reply: &str,
) {
    let mk_meta = || {
        let mut m = EventMeta::now();
        m.task = Some(task);
        m.agent = agent;
        m
    };
    bus.publish(Event {
        meta: mk_meta(),
        kind: EventKind::UserPrompted {
            text: user.to_string(),
        },
    })
    .expect("publish user");
    bus.publish(Event {
        meta: mk_meta(),
        kind: EventKind::AgentResponded {
            text: reply.to_string(),
        },
    })
    .expect("publish reply");
    bus.publish(Event {
        meta: mk_meta(),
        kind: EventKind::TaskStateChanged {
            from: TaskState::InProgress,
            to: TaskState::Done,
        },
    })
    .expect("publish done");
}

async fn wait_until<F>(cond: F)
where
    F: Fn() -> bool,
{
    // WHY 轮询：Extractor 里跑 tokio::spawn，测试要等它处理完事件；上限 2s。
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("wait_until 超时");
}

#[tokio::test]
async fn extractor_on_task_done_calls_spawner_with_transcript() {
    let (bus, oracle) = bus_and_oracle().await;
    let spawner = MockSpawner::with_script(vec![Ok("[]".to_string())]);
    let cfg = ExtractorConfig::default();
    let _handle = Extractor::new(bus.clone(), oracle.clone(), spawner.clone(), cfg).spawn();

    let task = TaskId::new();
    let agent = AgentId::new();
    spawner.set_role(agent, "dev").await;
    publish_task_transcript(&bus, task, Some(agent), "你好我爱喝冰美式", "记下了").await;

    wait_until(|| spawner.call_count() >= 1).await;
    let prompts = spawner.prompts().await;
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("你好我爱喝冰美式"));
    assert!(prompts[0].contains("记下了"));
}

#[tokio::test]
async fn extractor_parses_json_list_and_inserts_facts() {
    let (bus, oracle) = bus_and_oracle().await;
    let json = r#"[{"subject":"user","predicate":"name","object":"linda"}]"#;
    let spawner = MockSpawner::with_script(vec![Ok(json.to_string())]);
    let _handle = Extractor::new(
        bus.clone(),
        oracle.clone(),
        spawner.clone(),
        ExtractorConfig::default(),
    )
    .spawn();

    let task = TaskId::new();
    let agent = AgentId::new();
    spawner.set_role(agent, "pm").await;
    publish_task_transcript(&bus, task, Some(agent), "我叫 linda", "嗯").await;

    wait_until(|| spawner.call_count() >= 1).await;
    // 给 Extractor 一点写 oracle 的时间
    for _ in 0..100 {
        if oracle
            .query_one("user", "name")
            .await
            .expect("query")
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let got = oracle
        .query_one("user", "name")
        .await
        .expect("query")
        .expect("应有 user/name=linda");
    assert_eq!(got.object, "linda");
    assert_eq!(got.source, "extractor");
    assert!((got.confidence - 0.7).abs() < 1e-4);
}

#[tokio::test]
async fn extractor_skips_extractor_own_role() {
    // 防递归：如果 Done 的 task 本身是 extractor 门客的产物，不能再抽。
    let (bus, oracle) = bus_and_oracle().await;
    let spawner = MockSpawner::with_script(vec![Ok("[]".to_string())]);
    let _handle = Extractor::new(
        bus.clone(),
        oracle.clone(),
        spawner.clone(),
        ExtractorConfig::default(),
    )
    .spawn();

    let task = TaskId::new();
    let agent = AgentId::new();
    spawner.set_role(agent, "extractor").await;
    publish_task_transcript(&bus, task, Some(agent), "抽取什么东西", "[]").await;

    // 等足够长以便 Extractor 如果要调会调到——50ms 够了（其余场景 wait_until 10ms 就见效）。
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(spawner.call_count(), 0, "extractor 自己的 task 不应再触发");
}

#[tokio::test]
async fn extractor_caps_facts_at_max_per_task() {
    let (bus, oracle) = bus_and_oracle().await;
    // 构造 20 条 fact，但 cfg 限制 10。
    let mut facts = Vec::new();
    for i in 0..20 {
        facts.push(format!(
            r#"{{"subject":"user","predicate":"p{i}","object":"v{i}"}}"#
        ));
    }
    let json = format!("[{}]", facts.join(","));
    let spawner = MockSpawner::with_script(vec![Ok(json)]);
    let cfg = ExtractorConfig {
        max_facts_per_task: 10,
        ..ExtractorConfig::default()
    };
    let _handle = Extractor::new(bus.clone(), oracle.clone(), spawner.clone(), cfg).spawn();

    let task = TaskId::new();
    let agent = AgentId::new();
    spawner.set_role(agent, "dev").await;
    publish_task_transcript(&bus, task, Some(agent), "堆 fact", "yo").await;

    wait_until(|| spawner.call_count() >= 1).await;
    // 等落库：查 subject=user 总数最多 10。
    for _ in 0..200 {
        let rows = oracle.query("user", 100).await.expect("query");
        if rows.len() >= 10 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let rows = oracle.query("user", 100).await.expect("query");
    assert_eq!(rows.len(), 10, "熔断：每 task 最多 10 条");
}

#[tokio::test]
async fn extractor_handles_malformed_json_gracefully() {
    let (bus, oracle) = bus_and_oracle().await;
    let spawner = MockSpawner::with_script(vec![Ok("not json".to_string())]);
    let _handle = Extractor::new(
        bus.clone(),
        oracle.clone(),
        spawner.clone(),
        ExtractorConfig::default(),
    )
    .spawn();

    let task = TaskId::new();
    let agent = AgentId::new();
    spawner.set_role(agent, "dev").await;
    publish_task_transcript(&bus, task, Some(agent), "垃圾输入", "啥也不说").await;

    wait_until(|| spawner.call_count() >= 1).await;
    // 等 100ms，不应 panic，也不应有任何 fact 进库。
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rows = oracle.query("user", 100).await.expect("query");
    assert_eq!(rows.len(), 0, "坏 JSON 不应入库");
}
