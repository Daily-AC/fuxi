//! Fixture-driven 测试——用真实采样 JSONL 喂 parser + translator，
//! 验证完整对话的 Event 序列符合预期。不 spawn 真 codex。

use fuxi_agent_codex::{CodexEvent, TranslateState, parse_line, translate};
use fuxi_core::event::EventKind;
use fuxi_core::id::AgentId;
use fuxi_core::task::TaskState;

const FIXTURE: &str = include_str!("fixtures/codex_sample.jsonl");

#[test]
fn fixture_parses_line_by_line() {
    let mut labels = Vec::new();
    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let cx = parse_line(line).expect("parse fixture line");
        labels.push(codex_variant_label(cx));
    }
    assert_eq!(
        labels,
        vec![
            "ThreadStarted",
            "TurnStarted",
            "AgentMessage",
            "CommandStarted",
            "CommandCompleted",
            "AgentMessage",
            "TurnCompleted",
        ]
    );
}

#[test]
fn fixture_translates_to_expected_event_sequence() {
    let agent = AgentId::new();
    let mut state = TranslateState::new();
    let mut events: Vec<EventKind> = Vec::new();

    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let cx = parse_line(line).expect("parse");
        events.extend(
            translate(cx, agent, None, &mut state, Some(99))
                .into_iter()
                .map(|e| e.kind),
        );
    }

    // 最关键的断言：AgentReady 在最前；末尾带 Delivering→Done + usage Custom；
    // 中间至少有一条 AgentResponded 包含 "hi"（case-insensitive contains）。
    assert!(
        matches!(events[0], EventKind::AgentReady { .. }),
        "first event should be AgentReady, got {:?}",
        events[0]
    );

    let saw_hi = events.iter().any(|e| match e {
        EventKind::AgentResponded { text, .. } => text.to_lowercase().contains("hi"),
        _ => false,
    });
    assert!(saw_hi, "no AgentResponded containing 'hi' in {events:#?}");

    // 终态：倒数第二条是 TaskStateChanged Delivering→Done；最后一条是 usage Custom。
    let n = events.len();
    let EventKind::TaskStateChanged { from, to } = &events[n - 2] else {
        panic!(
            "expected TaskStateChanged at position n-2, got {:?}",
            events[n - 2]
        );
    };
    assert_eq!(*from, TaskState::Delivering);
    assert_eq!(*to, TaskState::Done);

    match &events[n - 1] {
        EventKind::Custom { label, .. } => assert_eq!(label, "codex_usage"),
        other => panic!("expected codex_usage Custom, got {other:?}"),
    }

    // AgentReady 必须带 pid endpoint（spawn 层注入了 pid_hint=99）。
    let EventKind::AgentReady { endpoint } = &events[0] else {
        unreachable!()
    };
    assert_eq!(endpoint, "pid:99");

    // shell 工具调用要成对出现：ToolCallStarted + ToolCallFinished。
    let started = events
        .iter()
        .filter(|e| matches!(e, EventKind::ToolCallStarted { .. }))
        .count();
    let finished = events
        .iter()
        .filter(|e| matches!(e, EventKind::ToolCallFinished { .. }))
        .count();
    assert_eq!(started, 1, "expected 1 ToolCallStarted");
    assert_eq!(finished, 1, "expected 1 ToolCallFinished");
}

fn codex_variant_label(cx: CodexEvent) -> &'static str {
    match cx {
        CodexEvent::ThreadStarted { .. } => "ThreadStarted",
        CodexEvent::TurnStarted => "TurnStarted",
        CodexEvent::CommandStarted { .. } => "CommandStarted",
        CodexEvent::CommandCompleted { .. } => "CommandCompleted",
        CodexEvent::AgentMessage { .. } => "AgentMessage",
        CodexEvent::ItemOther { .. } => "ItemOther",
        CodexEvent::TurnCompleted { .. } => "TurnCompleted",
        CodexEvent::TurnFailed { .. } => "TurnFailed",
        CodexEvent::Error { .. } => "Error",
        CodexEvent::Unknown { .. } => "Unknown",
    }
}
