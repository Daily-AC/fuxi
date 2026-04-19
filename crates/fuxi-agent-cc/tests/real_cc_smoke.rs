//! 真实 `claude` E2E 冒烟——会消耗 API 额度（~$0.05/次），默认跳过。
//!
//! 开启方式二选一：
//! - 环境变量：`FUXI_RUN_CC_E2E=1 cargo test -p fuxi-agent-cc --test real_cc_smoke -- --ignored --nocapture`
//! - cargo feature：`cargo test -p fuxi-agent-cc --features real_cc --test real_cc_smoke -- --ignored --nocapture`

use fuxi_agent_cc::{CcAgent, CcLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::EventKind;
use fuxi_core::task::Task;
use std::time::Duration;
use tokio::time::timeout;

fn should_run() -> bool {
    cfg!(feature = "real_cc") || std::env::var("FUXI_RUN_CC_E2E").is_ok()
}

#[tokio::test]
#[ignore = "spawns real claude and consumes API credits"]
async fn real_cc_echo_roundtrip() {
    if !should_run() {
        eprintln!("skipping: set FUXI_RUN_CC_E2E=1 or enable --features real_cc to run");
        return;
    }

    // 简单 trace——E2E 失败时人肉排障。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fuxi_agent_cc=debug,info".into()),
        )
        .try_init();

    let profile = AgentProfile {
        name: "cc-smoke".into(),
        role: "echo".into(),
        cli: "claude-code".into(),
        system_prompt: String::new(),
        tags: vec!["smoke".into()],
        extra: Default::default(),
    };
    // smoke 想省成本可手动 set FUXI_CC_MODEL=haiku，否则走 cc 默认（opus-4-7）
    let cfg = CcLaunchConfig::default();

    let agent = CcAgent::launch(profile, cfg).await.expect("launch claude");
    let task = Task::new("echo hi", "Reply with exactly: hi");
    let mut rx = agent.dispatch(task).await.expect("dispatch");

    let mut saw_ready = false;
    let mut saw_response_hi = false;
    let mut all_responses: Vec<String> = Vec::new();

    let outcome = timeout(Duration::from_secs(60), async {
        while let Some(ev) = rx.recv().await {
            match &ev.kind {
                EventKind::AgentReady { endpoint } => {
                    saw_ready = true;
                    eprintln!("AgentReady: {endpoint}");
                }
                EventKind::AgentResponded { text } => {
                    eprintln!("AgentResponded: {text:?}");
                    let normalized = text.trim().to_lowercase();
                    all_responses.push(text.clone());
                    if normalized == "hi" || normalized.contains("hi") {
                        saw_response_hi = true;
                    }
                }
                EventKind::TaskStateChanged { from, to } => {
                    eprintln!("TaskStateChanged: {from:?} -> {to:?}");
                    // WS 模式下 agent 长跑，rx 不会自动关——看到 Done 就显式退
                    if matches!(to, fuxi_core::task::TaskState::Done) {
                        break;
                    }
                }
                EventKind::TaskBlocked { reason } => {
                    panic!("task blocked by cc: {reason}");
                }
                EventKind::ToolCallStarted { tool, .. } => {
                    eprintln!("ToolCallStarted: {tool}");
                }
                EventKind::Custom { label, .. } => {
                    eprintln!("Custom: {label}");
                }
                other => {
                    eprintln!("event: {other:?}");
                }
            }
        }
    })
    .await;

    let _ = agent.shutdown().await;

    assert!(
        outcome.is_ok(),
        "E2E did not finish within 60s; responses so far: {all_responses:?}"
    );
    assert!(saw_ready, "did not see AgentReady event");
    assert!(
        saw_response_hi,
        "did not see 'hi' in any AgentResponded; got: {all_responses:?}"
    );
}
