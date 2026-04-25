//! cc stream-json 事件解析 + 翻译到 `fuxi_core::EventKind`。
//!
//! 为什么分两步（parse → translate）而不是直接 JSON → Event：
//! 1. cc 一条事件可能对应 0、1 或 2 条 fuxi 事件（例如 `result` 同时要发
//!    `TaskStateChanged` 和 `AgentResponded`）；
//! 2. thinking 需要「按块聚合」：多条 thinking delta 之间只该有 **一对**
//!    ThinkingStarted/Finished，而不是每 delta 一对；
//! 3. 中间类型 `CcEvent` 让单测不必造完整 JSON 也能验证翻译逻辑。
//!
//! 翻译规则表见 `reference_cc_stream_json.md` §cc↔A2A 事件映射。

use fuxi_core::event::{DeliverableKind, Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::TaskState;
use serde::Deserialize;
use serde_json::Value;

/// Decision 13 sentinel：LLM 在 `AssistantText` 内**单独一行**写出此 JSON object
/// 即触发 `AgentRequestReview`。`_fuxi` 命名空间隔离常规 JSON。
///
/// 防误触：parser 必须严格判 `text.trim().starts_with('{')`——markdown ``` ```
/// 围栏内的、缩进过的、引号包裹的 JSON 都不会被识别。
#[derive(Debug, Deserialize)]
struct RequestReviewSentinel {
    /// 必须等于 `"request_review"`——否则不是 fuxi 控制消息。
    #[serde(rename = "_fuxi")]
    kind_marker: String,
    kind: DeliverableKind,
    summary: String,
    #[serde(default)]
    artifact_ref: Option<String>,
}

/// 尝试把一段 `AssistantText` 解析为 sentinel；只有**整段单行裸 JSON object** +
/// `_fuxi == "request_review"` + `kind` 是合法枚举值 + `summary` 非空 才算命中。
/// 不命中（含解析失败）返 None，调用方退化为 `AgentResponded`。
fn try_parse_request_review_sentinel(text: &str) -> Option<RequestReviewSentinel> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let parsed: RequestReviewSentinel = serde_json::from_str(trimmed).ok()?;
    if parsed.kind_marker != "request_review" {
        return None;
    }
    if parsed.summary.is_empty() {
        return None;
    }
    Some(parsed)
}

/// 一条 stream-json 行被解析后的中间形态。
///
/// 只保留 fuxi 翻译用得到的字段；原始 JSON 存在 `raw` 兜底，方便
/// `Unknown` 变体透传给 `Custom`。
#[derive(Debug, Clone, PartialEq)]
pub enum CcEvent {
    /// `type:"system", subtype:"init"`。`pid_hint` 目前从 session_id 里借用；
    /// 真实 pid 由 spawn 层补齐到 `AgentReady` 事件里。
    SystemInit {
        session_id: String,
        model: Option<String>,
        cwd: Option<String>,
    },
    /// 其它 system 事件（hook_started 等）——`--bare` 下一般不出现，
    /// 走 Unknown 路径兜底。
    SystemOther { subtype: String, raw: Value },
    /// `type:"assistant"` 且 content 里有 `thinking` 块。
    AssistantThinking { text: String },
    /// `type:"assistant"` 且 content 里有 `text` 块。
    AssistantText { text: String },
    /// `type:"assistant"` 且 content 里有 `tool_use` 块。
    AssistantToolUse {
        tool_id: String,
        tool_name: String,
        input: Value,
    },
    /// `type:"user"` + `content:[{type:"tool_result", ...}]`。
    UserToolResult {
        tool_use_id: String,
        is_error: bool,
        content_preview: String,
    },
    /// `type:"rate_limit_event"`.
    RateLimit { info: Value },
    /// `type:"result", subtype:"success"`.
    ResultSuccess { text: String },
    /// `type:"result", subtype:"error"`.
    ResultError { reason: String },
    /// 兜底——未知类型不让 parser 崩，交给 translator 变 `Custom`。
    Unknown { raw: Value },
}

/// 翻译器内部状态——只需要跟踪「当前是否处在 thinking 块内」，用来去重
/// ThinkingStarted/Finished。
///
/// 为什么不直接用 `std::mem::replace`：thinking 块的结束边界是「下一条
/// 非-thinking 事件到来」——这个判断必须由调用方（agent 事件循环）驱动。
#[derive(Debug, Default)]
pub struct TranslateState {
    /// 当前处于 thinking 累积中。进入时推 `ThinkingStarted`；离开时推 `Finished`。
    in_thinking: bool,
    /// 本 turn 是否已发过 `AgentResponded`（走 `AssistantText` 路径）——
    /// 2026-04-20 修双发 bug：`ResultSuccess` 的 `text` 和最后一条
    /// `AssistantText` 内容一致，再发一次 `AgentResponded` 让 TUI 看到相同文本
    /// 两遍。新策略：`AssistantText` 发的同时置标；`ResultSuccess` 看标——
    /// 已发则跳过冗余文本；未发（极短响应只在 result 里给 text 的冷场景）才发。
    /// terminal 后由 pump 调 `finish()` reset 给下一 turn。
    responded_this_turn: bool,
}

impl TranslateState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 标记当前事件流已收尾（result 到达）——如果还挂在 thinking 里，
    /// 强制闭合。同时 reset `responded_this_turn` 给下一轮 turn 用。
    pub fn finish(&mut self) -> bool {
        let was = self.in_thinking;
        self.in_thinking = false;
        self.responded_this_turn = false;
        was
    }
}

/// 解析一行 stream-json 到 `CcEvent`。**不报错**——未知形态走 `Unknown` 兜底。
pub fn parse_line(line: &str) -> Result<CcEvent, serde_json::Error> {
    let v: Value = serde_json::from_str(line)?;
    Ok(classify(v))
}

fn classify(v: Value) -> CcEvent {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "system" => classify_system(v),
        "assistant" => classify_assistant(v),
        "user" => classify_user(v),
        "rate_limit_event" => CcEvent::RateLimit {
            info: v.get("rate_limit_info").cloned().unwrap_or(Value::Null),
        },
        "result" => classify_result(v),
        _ => CcEvent::Unknown { raw: v },
    }
}

fn classify_system(v: Value) -> CcEvent {
    let subtype = v
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if subtype == "init" {
        let session_id = v
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let model = v
            .get("model")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let cwd = v.get("cwd").and_then(Value::as_str).map(|s| s.to_string());
        CcEvent::SystemInit {
            session_id,
            model,
            cwd,
        }
    } else {
        CcEvent::SystemOther { subtype, raw: v }
    }
}

fn classify_assistant(v: Value) -> CcEvent {
    // message.content 可能是 [thinking] | [text] | [tool_use]。同一条 event 通常
    // 只含一种（cc 对内实现如此——实测不会混合）。保守处理：取第一块。
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array);
    let Some(arr) = content else {
        return CcEvent::Unknown { raw: v };
    };
    let Some(first) = arr.first() else {
        return CcEvent::Unknown { raw: v };
    };
    let block_ty = first.get("type").and_then(Value::as_str).unwrap_or("");
    match block_ty {
        "thinking" => CcEvent::AssistantThinking {
            text: first
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "text" => CcEvent::AssistantText {
            text: first
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "tool_use" => CcEvent::AssistantToolUse {
            tool_id: first
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            tool_name: first
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            input: first.get("input").cloned().unwrap_or(Value::Null),
        },
        _ => CcEvent::Unknown { raw: v },
    }
}

fn classify_user(v: Value) -> CcEvent {
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array);
    let Some(arr) = content else {
        return CcEvent::Unknown { raw: v };
    };
    let Some(first) = arr.first() else {
        return CcEvent::Unknown { raw: v };
    };
    if first.get("type").and_then(Value::as_str) != Some("tool_result") {
        return CcEvent::Unknown { raw: v };
    }
    let tool_use_id = first
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let is_error = first
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // content 可能是 string、[{type:text,text:...}]，或其它结构——一律拉成字符串预览。
    let preview = match first.get("content") {
        Some(Value::String(s)) => truncate_preview(s, 256),
        Some(other) => truncate_preview(&other.to_string(), 256),
        None => String::new(),
    };
    CcEvent::UserToolResult {
        tool_use_id,
        is_error,
        content_preview: preview,
    }
}

fn classify_result(v: Value) -> CcEvent {
    let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
    let text = v
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match subtype {
        "success" => CcEvent::ResultSuccess { text },
        _ => {
            let reason = if text.is_empty() {
                v.get("terminal_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("cc reported error")
                    .to_string()
            } else {
                text
            };
            CcEvent::ResultError { reason }
        }
    }
}

fn truncate_preview(s: &str, max: usize) -> String {
    // 按字符截断避免切断 UTF-8。
    let mut out = String::with_capacity(max.min(s.len()));
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            break;
        }
        out.push(ch);
    }
    out
}

/// 将一个 `CcEvent` 翻译成 0 或多个 `fuxi_core::Event`。纯函数——测试友好。
///
/// `state` 负责跨事件的状态（thinking 块边界、已发过的 init）。
pub fn translate(
    cc: CcEvent,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    state: &mut TranslateState,
    pid_hint: Option<u32>,
) -> Vec<Event> {
    let mut out: Vec<Event> = Vec::new();

    // 进入 thinking 逻辑：如果当前 cc 事件**不是** thinking，则关闭之前的块。
    let is_thinking = matches!(cc, CcEvent::AssistantThinking { .. });
    if state.in_thinking && !is_thinking {
        out.push(mk_event(agent_id, task_id, EventKind::ThinkingFinished));
        state.in_thinking = false;
    }

    match cc {
        CcEvent::SystemInit { session_id, .. } => {
            let endpoint = match pid_hint {
                Some(pid) => format!("pid:{pid}"),
                None => format!("session:{session_id}"),
            };
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::AgentReady { endpoint },
            ));
        }
        CcEvent::SystemOther { subtype, raw } => {
            tracing::warn!(subtype = %subtype, "cc system event with unknown subtype");
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::Custom {
                    label: "cc_system_other".to_string(),
                    payload: raw,
                },
            ));
        }
        CcEvent::AssistantThinking { text } => {
            if !state.in_thinking {
                out.push(mk_event(agent_id, task_id, EventKind::ThinkingStarted));
                state.in_thinking = true;
            }
            // 累积文本作为 Custom 的 payload，方便 Firehose 复刻当时的思考内容；
            // 主事件就是 Started/Finished 界碑。
            if !text.is_empty() {
                out.push(mk_event(
                    agent_id,
                    task_id,
                    EventKind::Custom {
                        label: "cc_thinking_delta".to_string(),
                        payload: serde_json::json!({ "text": text }),
                    },
                ));
            }
        }
        CcEvent::AssistantText { text } => {
            // Decision 13 sentinel：先尝试识别为 `_fuxi:request_review` 控制消息。
            // 命中：发 AgentRequestReview，**不**置 responded_this_turn（控制消息
            // 不算 LLM 回复，避免吞掉真正的回复 turn-end fallback）。
            // task_id 为 None 不该发 sentinel（无 task 关联无意义）；按 caller
            // 契约 dispatch 后必有 current_task。这里仍守护一下：无 task 时降级
            // AgentResponded 透传。
            if let Some(sentinel) = try_parse_request_review_sentinel(&text)
                && let Some(t) = task_id
            {
                out.push(mk_event(
                    agent_id,
                    task_id,
                    EventKind::AgentRequestReview {
                        agent: agent_id,
                        task: t,
                        deliverable_kind: sentinel.kind,
                        summary: sentinel.summary,
                        artifact_ref: sentinel.artifact_ref,
                    },
                ));
                return out;
            }
            state.responded_this_turn = true;
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::AgentResponded { text },
            ));
        }
        CcEvent::AssistantToolUse {
            tool_name, input, ..
        } => {
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::ToolCallStarted {
                    tool: tool_name,
                    args: input,
                },
            ));
        }
        CcEvent::UserToolResult {
            is_error,
            content_preview,
            tool_use_id,
        } => {
            // 我们没有从 tool_result 反查 tool 名——上游 ToolCallStarted 已写过，
            // 这里把 tool_use_id 放进 tool 字段便于配对。Firehose 层可 join。
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::ToolCallFinished {
                    tool: tool_use_id,
                    ok: !is_error,
                    output_preview: content_preview,
                },
            ));
        }
        CcEvent::RateLimit { info } => {
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::Custom {
                    label: "rate_limit".to_string(),
                    payload: info,
                },
            ));
        }
        CcEvent::ResultSuccess { text } => {
            // 任务命中终态——先发状态转移。
            // InProgress → Delivering → Done 是合法路径，但我们没有 Delivering
            // 的语义锚点，直接从 InProgress 推到 Done 会破坏 can_transition_to；
            // 取折中：发 Delivering→Done（上层已负责进入 Delivering）。
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::TaskStateChanged {
                    from: TaskState::Delivering,
                    to: TaskState::Done,
                },
            ));
            // 修双发 bug（2026-04-20）：`AssistantText` 已经把流式最后一段发成
            // AgentResponded 了；result 里的 `text` 是同内容的终态副本，再发
            // 一次会让 TUI 显示两遍。仅在本 turn 没发过 AgentResponded 的
            // 极端场景（cc 只给 result text 没给 assistant stream）才补发。
            if !state.responded_this_turn && !text.is_empty() {
                out.push(mk_event(
                    agent_id,
                    task_id,
                    EventKind::AgentResponded { text },
                ));
            }
        }
        CcEvent::ResultError { reason } => {
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::TaskBlocked { reason },
            ));
        }
        CcEvent::Unknown { raw } => {
            tracing::warn!(?raw, "cc_unknown_event");
            out.push(mk_event(
                agent_id,
                task_id,
                EventKind::Custom {
                    label: "cc_unknown_event".to_string(),
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
    use fuxi_core::event::EventKind;

    fn fresh_agent() -> AgentId {
        AgentId::new()
    }

    // ── parser 单元 ─────────────────────────────────────────────

    #[test]
    fn parse_system_init() {
        let line =
            r#"{"type":"system","subtype":"init","session_id":"abc","model":"haiku","cwd":"/tmp"}"#;
        let ev = parse_line(line).expect("parse");
        match ev {
            CcEvent::SystemInit {
                session_id,
                model,
                cwd,
            } => {
                assert_eq!(session_id, "abc");
                assert_eq!(model.as_deref(), Some("haiku"));
                assert_eq!(cwd.as_deref(), Some("/tmp"));
            }
            other => panic!("expected SystemInit, got {other:?}"),
        }
    }

    #[test]
    fn parse_system_other_subtype() {
        let line = r#"{"type":"system","subtype":"hook_started","hook_id":"x"}"#;
        let ev = parse_line(line).expect("parse");
        matches!(ev, CcEvent::SystemOther { .. })
            .then_some(())
            .expect("SystemOther");
    }

    #[test]
    fn parse_assistant_thinking() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#;
        match parse_line(line).expect("parse") {
            CcEvent::AssistantThinking { text } => assert_eq!(text, "hmm"),
            other => panic!("expected AssistantThinking, got {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_text() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        match parse_line(line).expect("parse") {
            CcEvent::AssistantText { text } => assert_eq!(text, "hi"),
            other => panic!("expected AssistantText, got {other:?}"),
        }
    }

    #[test]
    fn parse_assistant_tool_use() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"cmd":"ls"}}]}}"#;
        match parse_line(line).expect("parse") {
            CcEvent::AssistantToolUse {
                tool_id,
                tool_name,
                input,
            } => {
                assert_eq!(tool_id, "t1");
                assert_eq!(tool_name, "Bash");
                assert_eq!(input["cmd"], "ls");
            }
            other => panic!("expected AssistantToolUse, got {other:?}"),
        }
    }

    #[test]
    fn parse_user_tool_result_string_content() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"ok"}]}}"#;
        match parse_line(line).expect("parse") {
            CcEvent::UserToolResult {
                tool_use_id,
                is_error,
                content_preview,
            } => {
                assert_eq!(tool_use_id, "t1");
                assert!(!is_error);
                assert_eq!(content_preview, "ok");
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_user_tool_result_struct_content_gets_stringified() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"boom"}]}]}}"#;
        match parse_line(line).expect("parse") {
            CcEvent::UserToolResult {
                is_error,
                content_preview,
                ..
            } => {
                assert!(is_error);
                assert!(content_preview.contains("boom"));
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_user_tool_result_preview_is_truncated() {
        let long = "x".repeat(1024);
        let line = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"{long}"}}]}}}}"#
        );
        match parse_line(&line).expect("parse") {
            CcEvent::UserToolResult {
                content_preview, ..
            } => {
                assert_eq!(content_preview.chars().count(), 256);
            }
            other => panic!("expected UserToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_rate_limit() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        match parse_line(line).expect("parse") {
            CcEvent::RateLimit { info } => assert_eq!(info["status"], "allowed"),
            other => panic!("expected RateLimit, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_success() {
        let line = r#"{"type":"result","subtype":"success","result":"done"}"#;
        match parse_line(line).expect("parse") {
            CcEvent::ResultSuccess { text } => assert_eq!(text, "done"),
            other => panic!("expected ResultSuccess, got {other:?}"),
        }
    }

    #[test]
    fn parse_result_error() {
        let line = r#"{"type":"result","subtype":"error","result":"","terminal_reason":"timeout"}"#;
        match parse_line(line).expect("parse") {
            CcEvent::ResultError { reason } => assert_eq!(reason, "timeout"),
            other => panic!("expected ResultError, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_type() {
        let line = r#"{"type":"something_new","foo":"bar"}"#;
        match parse_line(line).expect("parse") {
            CcEvent::Unknown { raw } => assert_eq!(raw["foo"], "bar"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn parse_invalid_json_returns_err() {
        assert!(parse_line("{not json").is_err());
    }

    // ── translator 单元 ─────────────────────────────────────────

    #[test]
    fn translate_init_emits_agent_ready_with_pid() {
        let mut st = TranslateState::new();
        let agent = fresh_agent();
        let out = translate(
            CcEvent::SystemInit {
                session_id: "s".to_string(),
                model: None,
                cwd: None,
            },
            agent,
            None,
            &mut st,
            Some(42),
        );
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::AgentReady { endpoint } => assert_eq!(endpoint, "pid:42"),
            other => panic!("expected AgentReady, got {other:?}"),
        }
    }

    #[test]
    fn translate_init_falls_back_to_session_endpoint() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::SystemInit {
                session_id: "sess-xyz".to_string(),
                model: None,
                cwd: None,
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::AgentReady { endpoint } => assert_eq!(endpoint, "session:sess-xyz"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_thinking_emits_started_once_then_deltas() {
        let mut st = TranslateState::new();
        let agent = fresh_agent();
        let out1 = translate(
            CcEvent::AssistantThinking {
                text: "part1".into(),
            },
            agent,
            None,
            &mut st,
            None,
        );
        let out2 = translate(
            CcEvent::AssistantThinking {
                text: "part2".into(),
            },
            agent,
            None,
            &mut st,
            None,
        );
        // 首次：Started + Delta；第二次：只 Delta。
        assert!(matches!(out1[0].kind, EventKind::ThinkingStarted));
        assert!(matches!(out1[1].kind, EventKind::Custom { .. }));
        assert_eq!(out2.len(), 1);
        assert!(matches!(out2[0].kind, EventKind::Custom { .. }));
        assert!(st.in_thinking);
    }

    #[test]
    fn translate_non_thinking_after_thinking_closes_block() {
        let mut st = TranslateState::new();
        let agent = fresh_agent();
        translate(
            CcEvent::AssistantThinking { text: "t".into() },
            agent,
            None,
            &mut st,
            None,
        );
        let out = translate(
            CcEvent::AssistantText { text: "hi".into() },
            agent,
            None,
            &mut st,
            None,
        );
        // 收尾的 ThinkingFinished 先走，AgentResponded 再走。
        assert!(matches!(out[0].kind, EventKind::ThinkingFinished));
        assert!(matches!(out[1].kind, EventKind::AgentResponded { .. }));
        assert!(!st.in_thinking);
    }

    #[test]
    fn translate_tool_use_emits_started() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::AssistantToolUse {
                tool_id: "t".into(),
                tool_name: "Bash".into(),
                input: serde_json::json!({"cmd":"ls"}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::ToolCallStarted { tool, args } => {
                assert_eq!(tool, "Bash");
                assert_eq!(args["cmd"], "ls");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_tool_result_emits_finished_with_ok_flag() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::UserToolResult {
                tool_use_id: "t".into(),
                is_error: false,
                content_preview: "ok".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::ToolCallFinished {
                ok, output_preview, ..
            } => {
                assert!(*ok);
                assert_eq!(output_preview, "ok");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_rate_limit_goes_to_custom() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::RateLimit {
                info: serde_json::json!({"status":"allowed"}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::Custom { label, payload } => {
                assert_eq!(label, "rate_limit");
                assert_eq!(payload["status"], "allowed");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_result_success_emits_state_change_and_response() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::ResultSuccess { text: "hi".into() },
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
            EventKind::AgentResponded { text } => assert_eq!(text, "hi"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_result_error_emits_blocked() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::ResultError {
                reason: "api".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::TaskBlocked { reason } => assert_eq!(reason, "api"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_unknown_emits_custom() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::Unknown {
                raw: serde_json::json!({"foo":"bar"}),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        match &out[0].kind {
            EventKind::Custom { label, payload } => {
                assert_eq!(label, "cc_unknown_event");
                assert_eq!(payload["foo"], "bar");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn translate_result_after_thinking_closes_block_first() {
        let mut st = TranslateState::new();
        translate(
            CcEvent::AssistantThinking { text: "t".into() },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        let out = translate(
            CcEvent::ResultSuccess {
                text: "done".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        // ThinkingFinished + TaskStateChanged + AgentResponded
        assert_eq!(out.len(), 3);
        assert!(matches!(out[0].kind, EventKind::ThinkingFinished));
    }

    /// 双发 bug 回归保护（2026-04-20 用户复测发现）：
    /// 若一个 turn 里先 AssistantText 发了 AgentResponded，result 来时 text 是
    /// 同内容副本，**不应再发第二次 AgentResponded**——TUI 会显示两遍。
    #[test]
    fn translate_assistant_text_then_result_does_not_double_emit_responded() {
        let mut st = TranslateState::new();
        let out1 = translate(
            CcEvent::AssistantText {
                text: "hello world".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        // AssistantText 阶段应发 AgentResponded
        assert_eq!(out1.len(), 1);
        assert!(matches!(out1[0].kind, EventKind::AgentResponded { .. }));

        // terminal 阶段：cc 把最后那段文本又在 result 里重复一次
        let out2 = translate(
            CcEvent::ResultSuccess {
                text: "hello world".into(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        // 仅发 TaskStateChanged，不再 AgentResponded
        assert_eq!(
            out2.len(),
            1,
            "已有 AssistantText 的 turn 不应在 result 再发 AgentResponded"
        );
        assert!(matches!(out2[0].kind, EventKind::TaskStateChanged { .. }));
    }

    /// 冷场景保护：cc 某些极短响应只在 result 带 text 不发 assistant stream，
    /// 此时仍必须发 AgentResponded（否则 TUI 完全看不到回复）。
    #[test]
    fn translate_result_only_still_emits_responded_without_prior_assistant() {
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::ResultSuccess { text: "ok".into() },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        assert_eq!(out.len(), 2, "冷场景应发 TaskStateChanged + AgentResponded");
        assert!(matches!(out[1].kind, EventKind::AgentResponded { .. }));
    }

    /// `finish()` 必须 reset responded_this_turn——否则下一 turn 的 result
    /// （即使是冷场景）也会被误跳。
    #[test]
    fn finish_resets_responded_flag_for_next_turn() {
        let mut st = TranslateState::new();
        translate(
            CcEvent::AssistantText { text: "a".into() },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        st.finish();
        // 新 turn 冷启动：应正常发 AgentResponded
        let out = translate(
            CcEvent::ResultSuccess { text: "b".into() },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        assert_eq!(out.len(), 2, "finish reset 后应恢复冷场景的 AgentResponded");
        assert!(matches!(out[1].kind, EventKind::AgentResponded { .. }));
    }

    #[test]
    fn truncate_preview_preserves_utf8() {
        let s = "你好世界".repeat(200);
        let t = super::truncate_preview(&s, 256);
        // 不应崩溃，不应切坏字符——chars().count 必须 <= 256。
        assert!(t.chars().count() <= 256);
    }

    // ── sentinel marker `_fuxi:request_review`（Decision 13）────────

    /// 主路径：`AssistantText` 单独一行裸 JSON `{"_fuxi":"request_review", ...}`
    /// 必须翻译成 `AgentRequestReview`，**且不再 emit AgentResponded**——
    /// 把控制消息藏起来，玄女只看到 review 请求，用户也不会看到一坨 JSON。
    #[test]
    fn translate_sentinel_emits_request_review_and_suppresses_responded() {
        use fuxi_core::event::DeliverableKind;
        let mut st = TranslateState::new();
        let task = TaskId::new();
        let out = translate(
            CcEvent::AssistantText {
                text: r#"{"_fuxi":"request_review","kind":"code_change","summary":"小绿了","artifact_ref":"sha:abc"}"#
                    .to_string(),
            },
            fresh_agent(),
            Some(task),
            &mut st,
            None,
        );
        assert_eq!(
            out.len(),
            1,
            "sentinel 行只发一条事件，suppressed AgentResponded"
        );
        match &out[0].kind {
            EventKind::AgentRequestReview {
                deliverable_kind,
                summary,
                artifact_ref,
                task: t,
                ..
            } => {
                assert_eq!(*deliverable_kind, DeliverableKind::CodeChange);
                assert_eq!(summary, "小绿了");
                assert_eq!(artifact_ref.as_deref(), Some("sha:abc"));
                assert_eq!(*t, task);
            }
            other => panic!("expected AgentRequestReview, got {other:?}"),
        }
        // 关键：sentinel 不该置 responded_this_turn——终态时冷场景仍要发 result text。
        // （否则 LLM 一行 sentinel + 一行真回复在 result 里被吞）
        assert!(
            !st.responded_this_turn,
            "sentinel 行不属于 LLM 回复，不该置 responded_this_turn"
        );
    }

    /// `summary` 必填 + `kind` 必为枚举值之一；缺字段或 kind 非法 → 退化普通 AgentResponded。
    #[test]
    fn translate_sentinel_with_missing_kind_falls_back_to_responded() {
        let mut st = TranslateState::new();
        let bad = r#"{"_fuxi":"request_review","summary":"only summary"}"#;
        let out = translate(
            CcEvent::AssistantText {
                text: bad.to_string(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        // 没 kind = 不是合法 sentinel = 当普通文本走，TUI 仍能看到（用户 debug 友好）
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0].kind, EventKind::AgentResponded { text } if text == bad),
            "缺 kind 应退化 AgentResponded，got {:?}",
            out[0].kind
        );
    }

    /// 防误触防线 1：sentinel 必须**首字符 `{`**——markdown 代码块（前导
    /// ``` ```、缩进、引号包裹）里的 JSON 不应触发。
    #[test]
    fn translate_sentinel_in_code_fence_does_not_trigger() {
        let mut st = TranslateState::new();
        // 模拟 LLM 在示例文档里写「这条事件长这样：` ```{"_fuxi":"..."} ``` 」
        let fenced =
            "```\n{\"_fuxi\":\"request_review\",\"kind\":\"code_change\",\"summary\":\"x\"}\n```";
        let out = translate(
            CcEvent::AssistantText {
                text: fenced.to_string(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0].kind, EventKind::AgentResponded { .. }),
            "代码块整体不是行首 JSON，应当 AgentResponded 整段透传，got {:?}",
            out[0].kind
        );
    }

    /// 防误触防线 2：单行裸 JSON 但 `_fuxi` 字段不是 `"request_review"` —— 退化普通文本。
    #[test]
    fn translate_non_fuxi_json_passes_as_responded() {
        let mut st = TranslateState::new();
        let other_json = r#"{"hello":"world","kind":"code_change"}"#;
        let out = translate(
            CcEvent::AssistantText {
                text: other_json.to_string(),
            },
            fresh_agent(),
            None,
            &mut st,
            None,
        );
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0].kind, EventKind::AgentResponded { text } if text == other_json),
            "非 _fuxi sentinel 应当透传，got {:?}",
            out[0].kind
        );
    }

    /// `artifact_ref` 是 Optional——缺它仍是合法 sentinel（譬如 research_summary 类）。
    #[test]
    fn translate_sentinel_without_artifact_ref_is_valid() {
        use fuxi_core::event::DeliverableKind;
        let mut st = TranslateState::new();
        let out = translate(
            CcEvent::AssistantText {
                text:
                    r#"{"_fuxi":"request_review","kind":"research_summary","summary":"读完 auth"}"#
                        .to_string(),
            },
            fresh_agent(),
            Some(TaskId::new()),
            &mut st,
            None,
        );
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            EventKind::AgentRequestReview {
                deliverable_kind,
                artifact_ref,
                ..
            } => {
                assert_eq!(*deliverable_kind, DeliverableKind::ResearchSummary);
                assert!(artifact_ref.is_none());
            }
            other => panic!("expected AgentRequestReview, got {other:?}"),
        }
    }

    /// task_id 缺失时（dispatch 前的孤立场景）退化 AgentResponded 透传——
    /// AgentRequestReview 字段强制 `task: TaskId`，无 task 关联无意义。
    #[test]
    fn translate_sentinel_without_task_id_falls_back_to_responded() {
        let mut st = TranslateState::new();
        let raw =
            r#"{"_fuxi":"request_review","kind":"code_change","summary":"x","artifact_ref":null}"#;
        let out = translate(
            CcEvent::AssistantText {
                text: raw.to_string(),
            },
            fresh_agent(),
            None, // 无 task_id
            &mut st,
            None,
        );
        assert_eq!(out.len(), 1);
        assert!(
            matches!(&out[0].kind, EventKind::AgentResponded { text } if text == raw),
            "无 task_id 时应退化 AgentResponded 透传，got {:?}",
            out[0].kind
        );
    }
}
