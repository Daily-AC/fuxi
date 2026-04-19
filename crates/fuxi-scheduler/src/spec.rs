//! Trigger 规格——四种 kind 的参数合并结构。
//!
//! 存库时以 JSON 形式放在 `triggers.spec` 列；同时冗余一列 `kind` 便于 WHERE 过滤。
//!
//! 为什么外层用 `#[serde(tag = "kind")]`：DB 列 `kind` 就是这个 tag，JSON 本身自解释；
//! 加新变体时（例如 `Mqtt`），只需加 enum 成员，无需动 schema。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Trigger 的参数合并结构——每个 kind 一个 variant。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerSpec {
    /// 时刻表型——croner 表达式；支持 5/6 字段（后者含秒）。
    Cron {
        expr: String,
        /// 时区字符串（例 `"Asia/Shanghai"`）；缺省 UTC。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tz: Option<String>,
    },
    /// 一次性——绝对时间点；到期后 trigger 自动 `enabled=0`。
    Once { at: DateTime<Utc> },
    /// 文件系统监视——路径变动即 fire。
    FsWatch {
        path: PathBuf,
        /// notify 的事件类型白名单，例如 `["modify","create"]`；空数组 = 全部。
        #[serde(default)]
        events: Vec<String>,
    },
    /// Webhook——`POST /hook/<trigger_id>` 命中。
    Webhook {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
}

impl TriggerSpec {
    /// Trigger kind 的字符串形式——同 `triggers.kind` 列值。
    pub fn kind_str(&self) -> &'static str {
        match self {
            TriggerSpec::Cron { .. } => "cron",
            TriggerSpec::Once { .. } => "once",
            TriggerSpec::FsWatch { .. } => "fs_watch",
            TriggerSpec::Webhook { .. } => "webhook",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn cron_spec_roundtrip() {
        let s = TriggerSpec::Cron {
            expr: "0 */5 * * *".into(),
            tz: Some("Asia/Shanghai".into()),
        };
        let j = serde_json::to_value(&s).expect("serialize");
        assert_eq!(j["kind"], "cron");
        assert_eq!(j["expr"], "0 */5 * * *");
        assert_eq!(j["tz"], "Asia/Shanghai");
        let back: TriggerSpec = serde_json::from_value(j).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn cron_spec_tz_optional_omitted_when_none() {
        let s = TriggerSpec::Cron {
            expr: "*/2 * * * * *".into(),
            tz: None,
        };
        let j = serde_json::to_string(&s).expect("serialize");
        assert!(!j.contains("\"tz\""), "tz 应被 skip_serializing: {j}");
    }

    #[test]
    fn once_spec_roundtrip_with_tz() {
        let ts = Utc.with_ymd_and_hms(2026, 4, 19, 21, 0, 0).unwrap();
        let s = TriggerSpec::Once { at: ts };
        let j = serde_json::to_string(&s).expect("serialize");
        let back: TriggerSpec = serde_json::from_str(&j).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn fs_watch_spec_roundtrip() {
        let s = TriggerSpec::FsWatch {
            path: PathBuf::from("/tmp/fuxi-test"),
            events: vec!["modify".into(), "create".into()],
        };
        let j = serde_json::to_value(&s).expect("serialize");
        assert_eq!(j["kind"], "fs_watch");
        let back: TriggerSpec = serde_json::from_value(j).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn webhook_spec_roundtrip_without_secret() {
        let s = TriggerSpec::Webhook { secret: None };
        let j = serde_json::to_string(&s).expect("serialize");
        assert!(!j.contains("\"secret\""), "secret 缺省时不应序列化: {j}");
        let back: TriggerSpec = serde_json::from_str(&j).expect("deserialize");
        assert_eq!(back, s);
    }

    #[test]
    fn kind_str_matches_tag() {
        assert_eq!(
            TriggerSpec::Cron {
                expr: "* * * * *".into(),
                tz: None
            }
            .kind_str(),
            "cron"
        );
        assert_eq!(TriggerSpec::Once { at: Utc::now() }.kind_str(), "once");
        assert_eq!(
            TriggerSpec::FsWatch {
                path: PathBuf::from("/x"),
                events: vec![]
            }
            .kind_str(),
            "fs_watch"
        );
        assert_eq!(TriggerSpec::Webhook { secret: None }.kind_str(), "webhook");
    }
}
