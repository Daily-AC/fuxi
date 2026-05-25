//! 真实 `codex exec` E2E 冒烟——消耗 ChatGPT 账号额度，默认跳过。
//!
//! 开启方式二选一：
//! - 环境变量：`FUXI_RUN_CODEX_E2E=1 cargo test -p fuxi-agent-codex --test real_codex_smoke -- --ignored --nocapture`
//! - cargo feature：`cargo test -p fuxi-agent-codex --features real_codex --test real_codex_smoke -- --ignored --nocapture`

use fuxi_agent_codex::{CodexAgent, CodexLaunchConfig};
use fuxi_core::agent::{Agent, AgentProfile};
use fuxi_core::event::EventKind;
use fuxi_core::task::Task;
use std::time::Duration;
use tokio::time::timeout;

fn should_run() -> bool {
    cfg!(feature = "real_codex") || std::env::var("FUXI_RUN_CODEX_E2E").is_ok()
}

#[tokio::test]
#[ignore = "spawns real codex and consumes ChatGPT quota"]
async fn real_codex_echo_roundtrip() {
    if !should_run() {
        eprintln!("skipping: set FUXI_RUN_CODEX_E2E=1 or --features real_codex to run");
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fuxi_agent_codex=debug,info".into()),
        )
        .try_init();

    let profile = AgentProfile {
        name: "codex-smoke".into(),
        role: "echo".into(),
        cli: "codex".into(),
        system_prompt: String::new(),
        tags: vec!["smoke".into()],
        extra: Default::default(),
    };
    // 清空 model：让 codex 用它默认支持的模型，避免 `gpt-5.1-mini` 在 ChatGPT
    // 账号下被拒（实测报 invalid_request_error）。用户可通过 `FUXI_CODEX_MODEL`
    // 覆盖，下面若 env 有值就 honor。
    let model = std::env::var("FUXI_CODEX_MODEL").unwrap_or_default();
    let cfg = CodexLaunchConfig {
        model,
        // CI 或 smoke 场景要求完全无审批；full-auto 与 bypass 互斥，这里只开 bypass。
        full_auto: false,
        bypass_approvals: true,
        // E2E 可能跑在非 git 目录，`--skip-git-repo-check` 防御性兜底。
        extra_args: vec!["--skip-git-repo-check".into()],
        ..Default::default()
    };

    let agent = CodexAgent::launch(profile, cfg)
        .await
        .expect("launch codex");
    let task = Task::new("echo hi", "Reply with exactly the word hi then stop.");
    let mut rx = agent.dispatch(task).await.expect("dispatch");

    let mut saw_ready = false;
    let mut saw_response_hi = false;
    let mut saw_terminal = false;
    let mut responses: Vec<String> = Vec::new();

    let outcome = timeout(Duration::from_secs(60), async {
        while let Some(ev) = rx.recv().await {
            match &ev.kind {
                EventKind::AgentReady { endpoint } => {
                    saw_ready = true;
                    eprintln!("AgentReady: {endpoint}");
                }
                EventKind::AgentResponded { text, .. } => {
                    eprintln!("AgentResponded: {text:?}");
                    responses.push(text.clone());
                    if text.to_lowercase().contains("hi") {
                        saw_response_hi = true;
                    }
                }
                EventKind::TaskStateChanged { from, to } => {
                    eprintln!("TaskStateChanged: {from:?} -> {to:?}");
                    saw_terminal = true;
                }
                EventKind::TaskBlocked { reason } => {
                    panic!("task blocked by codex: {reason}");
                }
                EventKind::ToolCallStarted { tool, .. } => {
                    eprintln!("ToolCallStarted: {tool}");
                }
                EventKind::Custom { label, .. } => {
                    eprintln!("Custom: {label}");
                }
                other => eprintln!("event: {other:?}"),
            }
        }
    })
    .await;

    let _ = agent.shutdown().await;

    assert!(
        outcome.is_ok(),
        "E2E did not finish within 60s; responses so far: {responses:?}"
    );
    assert!(saw_ready, "did not see AgentReady event");
    assert!(saw_terminal, "did not see terminal TaskStateChanged");
    assert!(
        saw_response_hi,
        "did not see 'hi' in any AgentResponded; got: {responses:?}"
    );
}
