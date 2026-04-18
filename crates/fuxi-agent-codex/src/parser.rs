//! Codex `exec --json` 事件解析 + 翻译到 `fuxi_core::EventKind`。
//!
//! ## 采样得到的 codex JSONL 事件 schema（codex-cli 0.118）
//!
//! 每行一条 JSON 对象，顶层字段 `type` 区分种类。观察到的事件类型：
//!
//! | `type`            | 含义                          | 主要载荷字段                                                           |
//! | ----------------- | ----------------------------- | ---------------------------------------------------------------------- |
//! | `thread.started`  | 本次 thread 启动              | `thread_id`                                                            |
//! | `turn.started`    | 一轮推理开始                  | （空）                                                                 |
//! | `item.started`    | 某个 item 开始（通常是工具）  | `item.{id,type,...}`                                                   |
//! | `item.completed`  | 某个 item 完成                | `item.{id,type,...}`；`item.type` 可能是 `agent_message` 或 `command_execution`（也可能将来加别的） |
//! | `turn.completed`  | 一轮推理成功收尾              | `usage.{input_tokens,cached_input_tokens,output_tokens}`               |
//! | `turn.failed`     | 一轮推理失败                  | `error.message`                                                        |
//! | `error`           | 进程级错误（往往先于 `turn.failed`） | `message`                                                      |
//!
//! `item.type` 观察到的值：
//! - `agent_message`：模型回复文本（`item.text`）——翻译成 `AgentResponded`；
//! - `command_execution`：shell 工具调用（`item.command`/`aggregated_output`/`exit_code`/`status`）——
//!   translate 成 `ToolCallStarted`（在 `item.started` 时）/ `ToolCallFinished`（在 `item.completed` 时）。
//!
//! 目前**没有观察到** 独立的 "reasoning/thinking" 事件类型——
//! ChatGPT 账号下 gpt-5 系列的思考过程没有被 codex-cli 透出。所以
//! `ThinkingStarted/Finished` 目前不会被 emit。将来若 schema 扩展（例如
//! `item.type == "reasoning"`），在 `classify_item` 里加分支即可。
//!
//! 为什么 parse / translate 两步：
//! 1. 一条 codex 事件可能 → 0/1/2 条 fuxi 事件（`turn.completed` 要同时发
//!    `TaskStateChanged` 和最终 `AgentResponded` ——后者需要「记住」最近一次
//!    `agent_message` 的文本）；
//! 2. 中间类型 `CodexEvent` 让单测不用造 JSON 也能验证翻译逻辑。

use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::TaskState;
use serde_json::Value;

/// 一行 JSONL 被分类后的中间形态。
///
/// 只保留 fuxi 翻译用得到的字段；原始 JSON 存在 `raw` 兜底，供 `Unknown`。
#[derive(Debug, Clone, PartialEq)]
pub enum CodexEvent {
    /// `{"type":"thread.started","thread_id":"..."}`
    ThreadStarted { thread_id: String },
    /// `{"type":"turn.started"}`
    TurnStarted,
    /// `item.started` + `item.type == "command_execution"`。
    CommandStarted {
        item_id: String,
        command: String,
        raw_item: Value,
    },
    /// `item.completed` + `item.type == "command_execution"`。
    CommandCompleted {
        item_id: String,
        command: String,
        exit_code: Option<i64>,
        status: String,
        output_preview: String,
    },
    /// `item.completed` + `item.type == "agent_message"`。
    AgentMessage { item_id: String, text: String },
    /// `item.started` / `item.completed` 但 `item.type` 我们未识别。
    ItemOther {
        phase: ItemPhase,
        item_type: String,
        raw: Value,
    },
    /// `turn.completed` ——轮次成功。通常紧跟在 `agent_message` 之后。
    TurnCompleted { usage: Value },
    /// `turn.failed` ——轮次失败。
    TurnFailed { reason: String },
    /// `error` 事件（非 `turn.failed`）。
    Error { message: String },
    /// 其它未识别的顶层 type。
    Unknown { raw: Value },
}

/// 标记一个 item 事件是 started 还是 completed——仅给 Unknown/ItemOther 用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemPhase {
    Started,
    Completed,
}

/// 翻译器的跨事件状态。
///
/// 为什么需要：
/// 1. codex 的终态事件 `turn.completed` 不带最终文本——文本在此前的
///    `agent_message` item 里。我们需要缓存「最近一次 agent_message」，在
///    `turn.completed` 时回放为最终 `AgentResponded`。
/// 2. 未来若增加 thinking item type，同样可以在这里维护是否在 thinking 块。
#[derive(Debug, Default)]
pub struct TranslateState {
    /// 记住上一条 agent_message 的文本——turn.completed 时作为最终回复补发。
    /// 为什么不在 item 事件时就发：最终回复 _语义上_ 是「这个回合完了」的一部分，
    /// 上游 cc 适配器的等价做法是在 `result.success` 里带文本，我们对齐。
    last_agent_message: Option<String>,
}

impl TranslateState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 解析一行 JSONL 到 `CodexEvent`。未知形态走 `Unknown` 兜底，不报错。
pub fn parse_line(line: &str) -> Result<CodexEvent, serde_json::Error> {
    let v: Value = serde_json::from_str(line)?;
    Ok(classify(v))
}

fn classify(v: Value) -> CodexEvent {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "thread.started" => CodexEvent::ThreadStarted {
            thread_id: v
                .get("thread_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "turn.started" => CodexEvent::TurnStarted,
        "item.started" => classify_item(v, ItemPhase::Started),
        "item.completed" => classify_item(v, ItemPhase::Completed),
        "turn.completed" => CodexEvent::TurnCompleted {
            usage: v.get("usage").cloned().unwrap_or(Value::Null),
        },
        "turn.failed" => {
            let reason = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("codex turn failed")
                .to_string();
            CodexEvent::TurnFailed { reason }
        }
        "error" => CodexEvent::Error {
            message: v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("codex error")
                .to_string(),
        },
        _ => CodexEvent::Unknown { raw: v },
    }
}

fn classify_item(v: Value, phase: ItemPhase) -> CodexEvent {
    let Some(item) = v.get("item") else {
        return CodexEvent::Unknown { raw: v };
    };
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match item_type.as_str() {
        "agent_message" => {
            // agent_message 的 text 只在 completed 阶段出现；started 阶段我们
            // 当成 ItemOther，避免发出空 AgentResponded。
            if matches!(phase, ItemPhase::Completed) {
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                CodexEvent::AgentMessage { item_id, text }
            } else {
                CodexEvent::ItemOther {
                    phase,
                    item_type,
                    raw: v,
                }
            }
        }
        "command_execution" => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match phase {
                ItemPhase::Started => CodexEvent::CommandStarted {
                    item_id,
                    command,
                    raw_item: item.clone(),
                },
                ItemPhase::Completed => {
                    let exit_code = item.get("exit_code").and_then(Value::as_i64);
                    let status = item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let output_preview = item
                        .get("aggregated_output")
                        .and_then(Value::as_str)
                        .map(|s| truncate_preview(s, 256))
                        .unwrap_or_default();
                    CodexEvent::CommandCompleted {
                        item_id,
                        command,
                        exit_code,
                        status,
                        output_preview,
                    }
                }
            }
        }
        _ => CodexEvent::ItemOther {
            phase,
            item_type,
            raw: v,
        },
    }
}

fn truncate_preview(s: &str, max: usize) -> String {
    // 按字符截断，避免切坏 UTF-8。
    let mut out = String::with_capacity(max.min(s.len()));
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            break;
        }
        out.push(ch);
    }
    out
}

/// 把一个 `CodexEvent` 翻译成 0/1/多条 `fuxi_core::Event`。纯函数——测试友好。
///
/// `state` 负责跨事件的状态（缓存最近 agent_message 文本）。
pub fn translate(
    cx: CodexEvent,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    state: &mut TranslateState,
    pid_hint: Option<u32>,
) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::new();

    match cx {
        CodexEvent::ThreadStarted { thread_id } => {
            let endpoint = match pid_hint {
                Some(pid) => format!("pid:{pid}"),
                None => format!("thread:{thread_id}"),
            };
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::AgentReady { endpoint },
            ));
        }
        CodexEvent::TurnStarted => {
            // codex 不发独立的 reasoning 事件；turn.started 就是推理启动的唯一界碑，
            // 映射为 ThinkingStarted。与 cc 的 thinking delta 语义有偏差——codex 没文本——
            // 但保证 Firehose 看到「模型在思考」的时间锚点。
            out.push(mk_event(agent_id, task_id, EventKind::ThinkingStarted));
        }
        CodexEvent::CommandStarted {
            command, raw_item, ..
        } => {
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::ToolCallStarted {
                    tool: "shell".to_string(),
                    args: serde_json::json!({
                        "command": command,
                        "item": raw_item,
                    }),
                },
            ));
        }
        CodexEvent::CommandCompleted {
            command,
            exit_code,
            status,
            output_preview,
            ..
        } => {
            let ok = match exit_code {
                Some(0) => true,
                Some(_) => false,
                None => status == "completed",
            };
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::ToolCallFinished {
                    tool: format!("shell: {command}"),
                    ok,
                    output_preview,
                },
            ));
        }
        CodexEvent::AgentMessage { text, .. } => {
            // 缓存以便 turn.completed 时回放为最终 AgentResponded。
            // 但这里也先发一条 AgentResponded——Firehose 读者需要实时看到文本，
            // 而不是等整轮结束。turn.completed 不再重复发，只发状态转移。
            state.last_agent_message = Some(text.clone());
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::AgentResponded { text },
            ));
        }
        CodexEvent::ItemOther {
            phase,
            item_type,
            raw,
        } => {
            let label = format!(
                "codex_item_{}_{item_type}",
                match phase {
                    ItemPhase::Started => "started",
                    ItemPhase::Completed => "completed",
                }
            );
            tracing::debug!(%item_type, ?phase, "codex item with unhandled type");
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::Custom {
                    label,
                    payload: raw,
                },
            ));
        }
        CodexEvent::TurnCompleted { usage } => {
            // 终态：Delivering → Done。和 cc 一致——上层进入 Delivering 由编排层负责，
            // 我们只发允许的转移边。
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::TaskStateChanged {
                    from: TaskState::Delivering,
                    to: TaskState::Done,
                },
            ));
            // Usage 归档走 Custom——Firehose 读者可以统计 tokens。
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::Custom {
                    label: "codex_usage".to_string(),
                    payload: usage,
                },
            ));
        }
        CodexEvent::TurnFailed { reason } => {
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::TaskBlocked { reason },
            ));
        }
        CodexEvent::Error { message } => {
            // `error` 事件通常紧跟一条 `turn.failed`——这里也发 TaskBlocked，
            // 上层对「同一错误两次 Blocked」无语义冲突（最新一次覆盖），比漏报安全。
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::TaskBlocked { reason: message },
            ));
        }
        CodexEvent::Unknown { raw } => {
            tracing::warn!(?raw, "codex_unknown_event");
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::Custom {
                    label: "codex_unknown_event".to_string(),
                    payload: raw,
                },
            ));
        }
    }

    out
}

fn mk_event(agent: AgentId, task: Option<TaskId>, kind: EventKind) -> Event {
    let mut meta = EventMeta::now();
    meta.agent = Some(agent);
    meta.task = task;
    Event { meta, kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_agent() -> AgentId {
        AgentId::new()
    }

    // ── parser 单元 ─────────────────────────────────────────────

    #[test]
    fn parse_thread_started() {
        let line = r#"{"type":"thread.started","thread_id":"abc-123"}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::ThreadStarted { thread_id } => assert_eq!(thread_id, "abc-123"),
            other => panic!("expected ThreadStarted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_started() {
        let line = r#"{"type":"turn.started"}"#;
        assert_eq!(parse_line(line).expect("parse"), CodexEvent::TurnStarted);
    }

    #[test]
    fn parse_agent_message_completed() {
        let line = r#"{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hi"}}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::AgentMessage { item_id, text } => {
                assert_eq!(item_id, "item_0");
                assert_eq!(text, "hi");
            }
            other => panic!("expected AgentMessage, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_message_started_goes_to_item_other() {
        let line = r#"{"type":"item.started","item":{"id":"item_0","type":"agent_message"}}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::ItemOther {
                phase, item_type, ..
            } => {
                assert_eq!(phase, ItemPhase::Started);
                assert_eq!(item_type, "agent_message");
            }
            other => panic!("expected ItemOther, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_started() {
        let line = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"ls","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::CommandStarted {
                item_id, command, ..
            } => {
                assert_eq!(item_id, "item_1");
                assert_eq!(command, "ls");
            }
            other => panic!("expected CommandStarted, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_completed_success() {
        let line = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"ls","aggregated_output":"main.rs\n","exit_code":0,"status":"completed"}}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::CommandCompleted {
                exit_code,
                status,
                output_preview,
                ..
            } => {
                assert_eq!(exit_code, Some(0));
                assert_eq!(status, "completed");
                assert!(output_preview.contains("main.rs"));
            }
            other => panic!("expected CommandCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_completed_truncates_long_output() {
        let long = "x".repeat(1024);
        let line = format!(
            r#"{{"type":"item.completed","item":{{"id":"i","type":"command_execution","command":"c","aggregated_output":"{long}","exit_code":0,"status":"completed"}}}}"#
        );
        match parse_line(&line).expect("parse") {
            CodexEvent::CommandCompleted { output_preview, .. } => {
                assert_eq!(output_preview.chars().count(), 256);
            }
            other => panic!("expected CommandCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_completed_with_usage() {
        let line = r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":5}}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::TurnCompleted { usage } => {
                assert_eq!(usage["output_tokens"], 5);
            }
            other => panic!("expected TurnCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parse_turn_failed() {
        let line = r#"{"type":"turn.failed","error":{"message":"bad model"}}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::TurnFailed { reason } => assert_eq!(reason, "bad model"),
            other => panic!("expected TurnFailed, got {other:?}"),
        }
    }

    #[test]
    fn parse_error_event() {
        let line = r#"{"type":"error","message":"auth required"}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::Error { message } => assert_eq!(message, "auth required"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_type_goes_to_unknown() {
        let line = r#"{"type":"something_new","foo":"bar"}"#;
        match parse_line(line).expect("parse") {
            CodexEvent::Unknown { raw } => assert_eq!(raw["foo"], "bar"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn parse_invalid_json_returns_err() {
        assert!(parse_line("{not json").is_err());
    }

    #[test]
    fn parse_item_without_item_object_is_unknown() {
        let line = r#"{"type":"item.completed"}"#;
        matches!(parse_line(line).expect("parse"), CodexEvent::Unknown { .. })
            .then_some(())
            .expect("Unknown");
    }

    // ── translator 单元 ─────────────────────────────────────────

    #[test]
    fn translate_thread_started_emits_agent_ready_with_pid() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::ThreadStarted {
                thread_id: "t".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            Some(42),
        );
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::AgentReady { endpoint } => assert_eq!(endpoint, "pid:42"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_thread_started_falls_back_to_thread_id() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::ThreadStarted {
                thread_id: "abc".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::AgentReady { endpoint } => assert_eq!(endpoint, "thread:abc"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_turn_started_emits_thinking_started() {
        let mut st = TranslateState::new();
        let out = translate(CodexEvent::TurnStarted, fresh_agent(), None, &mut st, None);
        assert!(matches!(out[0].kind, EventKind::ThinkingStarted));
    }

    #[test]
    fn translate_agent_message_emits_agent_responded_and_caches_text() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::AgentMessage {
                item_id: "x".into(),
                text: "hi".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::AgentResponded { text } => assert_eq!(text, "hi"),
            other => panic!("got {other:?}"),
        }
        assert_eq!(st.last_agent_message.as_deref(), Some("hi"));
    }

    #[test]
    fn translate_command_started_emits_tool_call_started() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::CommandStarted {
                item_id: "i".into(),
                command: "ls -la".into(),
                raw_item: serde_json::json!({"command":"ls -la"}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::ToolCallStarted { tool, args } => {
                assert_eq!(tool, "shell");
                assert_eq!(args["command"], "ls -la");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_command_completed_success_marks_ok_true() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::CommandCompleted {
                item_id: "i".into(),
                command: "ls".into(),
                exit_code: Some(0),
                status: "completed".into(),
                output_preview: "main.rs".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::ToolCallFinished {
                tool,
                ok,
                output_preview,
            } => {
                assert!(*ok);
                assert!(tool.contains("ls"));
                assert_eq!(output_preview, "main.rs");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_command_completed_nonzero_is_not_ok() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::CommandCompleted {
                item_id: "i".into(),
                command: "false".into(),
                exit_code: Some(1),
                status: "completed".into(),
                output_preview: String::new(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::ToolCallFinished { ok, .. } => assert!(!*ok),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_turn_completed_emits_state_change_and_usage_custom() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::TurnCompleted {
                usage: serde_json::json!({"output_tokens":5}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        assert_eq!(out.len(), 2);
        match &out[0].kind {
            EventKind::TaskStateChanged { from, to } => {
                assert_eq!(*from, TaskState::Delivering);
                assert_eq!(*to, TaskState::Done);
            }
            other => panic!("got {other:?}"),
        }
        match &out[1].kind {
            EventKind::Custom { label, payload } => {
                assert_eq!(label, "codex_usage");
                assert_eq!(payload["output_tokens"], 5);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_turn_failed_emits_blocked() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::TurnFailed {
                reason: "boom".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::TaskBlocked { reason } => assert_eq!(reason, "boom"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_error_emits_blocked() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::Error {
                message: "auth".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::TaskBlocked { reason } => assert_eq!(reason, "auth"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_unknown_emits_custom_codex_unknown_event() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::Unknown {
                raw: serde_json::json!({"foo":"bar"}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::Custom { label, payload } => {
                assert_eq!(label, "codex_unknown_event");
                assert_eq!(payload["foo"], "bar");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_item_other_uses_descriptive_label() {
        let mut st = TranslateState::new();
        let out = translate(
            CodexEvent::ItemOther {
                phase: ItemPhase::Completed,
                item_type: "novel_kind".into(),
                raw: serde_json::json!({"foo":"bar"}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::Custom { label, .. } => {
                assert_eq!(label, "codex_item_completed_novel_kind");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn truncate_preview_preserves_utf8() {
        let s = "你好世界".repeat(200);
        let t = super::truncate_preview(&s, 256);
        assert!(t.chars().count() <= 256);
    }
}
