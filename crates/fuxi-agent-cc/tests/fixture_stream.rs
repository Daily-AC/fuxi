//! Fixture-driven 测试——用真实采样 stream-json 行喂 parser + translator，
//! 验证完整对话的 Event 序列符合预期。不 spawn 真 claude。

use fuxi_agent_cc::{CcEvent, TranslateState, parse_line, translate};
use fuxi_core::event::EventKind;
use fuxi_core::id::AgentId;
use fuxi_core::task::TaskState;

const FIXTURE: &str = include_str!("fixtures/stream_trimmed.jsonl");

#[test]
fn fixture_parses_line_by_line() {
    let mut kinds = Vec::new();
    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let cc = parse_line(line).expect("parse fixture line");
        kinds.push(std::mem::discriminant(&cc));
    }
    // 6 行 → 6 个 CcEvent。
    assert_eq!(kinds.len(), 6);
}

#[test]
fn fixture_translates_to_expected_event_sequence() {
    let agent = AgentId::new();
    let mut state = TranslateState::new();
    let mut events: Vec<EventKind> = Vec::new();

    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let cc = parse_line(line).expect("parse");
        let produced = translate(cc, agent, None, &mut state, Some(99));
        events.extend(produced.into_iter().map(|e| e.kind));
    }

    // 预期顺序：
    //   AgentReady                      (system.init)
    //   ThinkingStarted                 (first thinking)
    //   Custom(cc_thinking_delta)
    //   Custom(cc_thinking_delta)       (second thinking appended; no new Started)
    //   ThinkingFinished                (closed by following text)
    //   AgentResponded                  (assistant.text)
    //   Custom(rate_limit)
    //   TaskStateChanged                (result.success)
    //   AgentResponded                  (final result text)
    let labels: Vec<String> = events
        .iter()
        .map(|k| match k {
            EventKind::AgentReady { .. } => "AgentReady".to_string(),
            EventKind::ThinkingStarted => "ThinkingStarted".to_string(),
            EventKind::ThinkingFinished => "ThinkingFinished".to_string(),
            EventKind::AgentResponded { .. } => "AgentResponded".to_string(),
            EventKind::TaskStateChanged { .. } => "TaskStateChanged".to_string(),
            EventKind::TaskBlocked { .. } => "TaskBlocked".to_string(),
            EventKind::ToolCallStarted { .. } => "ToolCallStarted".to_string(),
            EventKind::ToolCallFinished { .. } => "ToolCallFinished".to_string(),
            EventKind::Custom { label, .. } => match label.as_str() {
                "cc_thinking_delta" => "ThinkingDelta".to_string(),
                "rate_limit" => "RateLimit".to_string(),
                other => other.to_string(),
            },
            _ => "Other".to_string(),
        })
        .collect();

    let expected: Vec<String> = vec![
        "AgentReady",
        "ThinkingStarted",
        "ThinkingDelta",
        "ThinkingDelta",
        "ThinkingFinished",
        "AgentResponded",
        "RateLimit",
        "TaskStateChanged",
        "AgentResponded",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        labels, expected,
        "full event sequence mismatch: {events:#?}"
    );

    // AgentReady 必须带 pid endpoint（spawn 层注入）。
    let EventKind::AgentReady { endpoint } = &events[0] else {
        unreachable!();
    };
    assert_eq!(endpoint, "pid:99");

    // 最终 AgentResponded 文本是 "hi"。
    let EventKind::AgentResponded { text } = events.last().expect("final event") else {
        panic!(
            "last event should be AgentResponded, got {:?}",
            events.last()
        );
    };
    assert_eq!(text.trim(), "hi");

    // TaskStateChanged 是 Delivering → Done。
    let EventKind::TaskStateChanged { from, to } = &events[events.len() - 2] else {
        panic!("penultimate should be TaskStateChanged");
    };
    assert_eq!(*from, TaskState::Delivering);
    assert_eq!(*to, TaskState::Done);
}

#[test]
fn classify_covers_all_fixture_variants() {
    let variants: Vec<&'static str> = FIXTURE
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| cc_variant_label(parse_line(line).expect("parse")))
        .collect();
    assert_eq!(
        variants,
        vec![
            "SystemInit",
            "AssistantThinking",
            "AssistantThinking",
            "AssistantText",
            "RateLimit",
            "ResultSuccess",
        ]
    );
}

fn cc_variant_label(cc: CcEvent) -> &'static str {
    match cc {
        CcEvent::SystemInit { .. } => "SystemInit",
        CcEvent::SystemOther { .. } => "SystemOther",
        CcEvent::AssistantThinking { .. } => "AssistantThinking",
        CcEvent::AssistantText { .. } => "AssistantText",
        CcEvent::AssistantToolUse { .. } => "AssistantToolUse",
        CcEvent::UserToolResult { .. } => "UserToolResult",
        CcEvent::RateLimit { .. } => "RateLimit",
        CcEvent::ResultSuccess { .. } => "ResultSuccess",
        CcEvent::ResultError { .. } => "ResultError",
        CcEvent::Unknown { .. } => "Unknown",
    }
}
