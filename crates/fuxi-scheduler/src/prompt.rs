//! 三段式唤醒 prompt —— trigger fire 后给玄女发的内容。
//!
//! 契约（见 `docs/architecture-v1.md` §M1.3）：
//! ```text
//! [TRIGGER_FIRED id=<uuid> fired_at=<iso8601> cause=<scheduled|webhook|fs|manual>]
//!
//! <triggers.intent 原句>
//!
//! [INSTRUCTION: 判断当前环境是否适合执行此触发。适合则调度门客，不适合则回报原因]
//! ```

use chrono::{DateTime, Utc};

/// 构造三段式 prompt。调用方已持有 id / fired_at / cause / intent。
pub fn build_trigger_prompt(
    id: &str,
    fired_at: DateTime<Utc>,
    cause: &str,
    intent: &str,
) -> String {
    format!(
        "[TRIGGER_FIRED id={id} fired_at={ts} cause={cause}]\n\n{intent}\n\n[INSTRUCTION: 判断当前环境是否适合执行此触发。适合则调度门客，不适合则回报原因]",
        ts = fired_at.to_rfc3339(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn prompt_has_all_three_stanzas() {
        let id = "trg_abc123";
        let at = Utc.with_ymd_and_hms(2026, 4, 19, 9, 0, 0).unwrap();
        let intent = "每周五早上 9 点，review 我这周的 PR";
        let s = build_trigger_prompt(id, at, "scheduled", intent);
        assert!(s.contains("[TRIGGER_FIRED id=trg_abc123"), "head: {s}");
        assert!(s.contains("cause=scheduled"), "cause: {s}");
        assert!(s.contains("fired_at=2026-04-19T09:00:00+00:00"), "ts: {s}");
        assert!(s.contains(intent), "intent: {s}");
        assert!(s.contains("[INSTRUCTION:"), "instruction: {s}");
    }

    #[test]
    fn prompt_blank_lines_between_stanzas() {
        let s = build_trigger_prompt(
            "trg_x",
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            "manual",
            "干活",
        );
        let parts: Vec<&str> = s.split("\n\n").collect();
        assert_eq!(parts.len(), 3, "三段之间应有空行: {s}");
        assert!(parts[0].starts_with("[TRIGGER_FIRED"));
        assert_eq!(parts[1], "干活");
        assert!(parts[2].starts_with("[INSTRUCTION:"));
    }

    #[test]
    fn prompt_handles_multiline_intent() {
        let intent = "第一行\n第二行\n第三行";
        let s = build_trigger_prompt(
            "trg_x",
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            "fs",
            intent,
        );
        // 多行 intent 要完整保留，不被截断
        assert!(s.contains("第一行\n第二行\n第三行"));
    }
}
