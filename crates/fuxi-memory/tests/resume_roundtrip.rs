//! 策府 + cc `--resume` 端到端联调（gated）。
//!
//! 开启方式：
//! ```bash
//! FUXI_RUN_MEMORY_E2E=1 cargo test -p fuxi-memory --test resume_roundtrip -- --ignored --nocapture
//! ```
//!
//! 过程：
//! 1. 玄女存一条偏好事实 + `session_id` 到策府
//! 2. 启门客一 (session_id=X)：告诉它「记住我爱喝冰美式」，等 Task Done
//! 3. 启门客二 (resume_session_id=X)：问「我刚说我爱喝啥？」
//! 4. 门客二的回复里应出现 "冰美式"——说明 `--resume` 真的续上了上下文
//!
//! 会消耗 API 额度（~$0.10），因此默认 #[ignore]。

use fuxi_agent_cc::{CcAgent, CcLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::EventKind;
use fuxi_core::task::{Task, TaskState};
use fuxi_memory::{NewFact, OracleStore};
use std::time::Duration;
use tokio::time::timeout;

fn should_run() -> bool {
    std::env::var("FUXI_RUN_MEMORY_E2E").is_ok()
}

fn make_profile() -> AgentProfile {
    AgentProfile {
        name: "xuannv-resume".into(),
        role: "xuannv".into(),
        cli: "claude-code".into(),
        system_prompt: String::new(),
        tags: vec!["memory".into(), "e2e".into()],
        extra: Default::default(),
    }
}

/// 起门客、发一条 prompt、等任务 Done（或超时），收集全部 `AgentResponded` 文本。
async fn one_turn(cfg: CcLaunchConfig, prompt: &str, title: &str) -> String {
    let profile = make_profile();
    let agent = CcAgent::launch(profile, cfg).await.expect("launch cc");
    let task = Task::new(title, prompt);
    let mut rx = agent.dispatch(task).await.expect("dispatch");

    let mut collected = String::new();
    let outcome = timeout(Duration::from_secs(90), async {
        while let Some(ev) = rx.recv().await {
            match &ev.kind {
                EventKind::AgentResponded { text, .. } => {
                    collected.push_str(text);
                    collected.push('\n');
                }
                EventKind::TaskStateChanged {
                    to: TaskState::Done,
                    ..
                } => break,
                EventKind::TaskBlocked { reason } => {
                    panic!("task blocked: {reason}");
                }
                _ => {}
            }
        }
    })
    .await;
    let _ = agent.shutdown().await;
    assert!(
        outcome.is_ok(),
        "E2E turn did not finish within 90s; partial: {collected:?}"
    );
    collected
}

#[tokio::test]
#[ignore = "spawns real claude twice; ~$0.10 API spend"]
async fn xuannv_remembers_preference_across_resume() {
    if !should_run() {
        eprintln!("skipping: set FUXI_RUN_MEMORY_E2E=1 to run");
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fuxi_agent_cc=info,fuxi_memory=debug".into()),
        )
        .try_init();

    // 1) 建一个临时策府，存一次对 session_id 的约定（后面两次 turn 都拿它）
    let store = OracleStore::connect_memory().await.unwrap();
    let fixed_session = uuid::Uuid::new_v4().to_string();
    store
        .insert(NewFact::new("xuannv", "session_id", &fixed_session).with_source("e2e"))
        .await
        .unwrap();
    // 读回来——确认策府 round trip
    let got = store.query_one("xuannv", "session_id").await.unwrap();
    assert_eq!(got.unwrap().object, fixed_session);

    // 2) 第一轮：给玄女塞记忆
    let first = CcLaunchConfig {
        session_id: Some(fixed_session.clone()),
        ..Default::default()
    };
    let r1 = one_turn(first, "记住我爱喝冰美式。回答 OK 即可。", "save-pref").await;
    eprintln!("== turn 1 transcript ==\n{r1}");

    // 3) 第二轮：同 session_id 走 --resume
    let second = CcLaunchConfig {
        resume_session_id: Some(fixed_session.clone()),
        ..Default::default()
    };
    let r2 = one_turn(second, "我刚说我爱喝什么？用一句话回答。", "ask-pref").await;
    eprintln!("== turn 2 transcript ==\n{r2}");

    assert!(
        r2.contains("冰美式"),
        "续写的门客没记住前轮偏好，回答里看不到目标短语。全文：\n{r2}"
    );
}
