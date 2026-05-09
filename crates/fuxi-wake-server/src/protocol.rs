//! Wake 协议 wire 类型——`apps/jarvis/WAKE_PROTOCOL.md` 是真相源。
//!
//! 上下行都用 `#[serde(tag = "type")]` 标签联合；二进制音频帧不进 enum，
//! 由 server WS handler 单独走 `Message::Binary` 分支。
//!
//! 命名约定：tag 用 snake_case（与 fuxi `EventKind` 同款 wire 形）。
//! 若改变 tag 名，Mac 端 `WireMessage` Codable 也要同步——是跨进程契约。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 下行（home → mac）控制消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// 收到 hello 后回——告知 SDK session 已就绪可以收音频。
    Ready { keywords: Vec<String> },
    /// 唤醒命中。`at` 用 RFC3339；rfc3339 串自动来自 chrono serde。
    Wake {
        keyword: String,
        score: f32,
        at: DateTime<Utc>,
    },
    /// 服务端心跳，5 秒一次。
    Ping { at: DateTime<Utc> },
    /// 错误下发——code 为契约词（`unauthorized` / `sdk_unavailable` /
    /// `audio_format_invalid` / `rate_limited`）。
    Error { code: String, message: String },
    /// 服务端主动断（升级 / 重启）。
    Bye,
}

/// 上行（mac → home）控制消息。二进制音频帧另走 `Message::Binary`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// 第一帧；server 收到后初始化 SDK session。
    Hello { client: String, version: String },
    /// 心跳响应。
    Pong { at: DateTime<Utc> },
    /// v0.1 server 写死 `["玄女"]`，先不实现切换；解析仍接收。
    Keywords { words: Vec<String> },
    /// 客户端主动下线。
    Bye,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 下行 ready：tag = "ready"，keywords 字段保留。
    #[test]
    fn server_ready_roundtrip() {
        let m = ServerMessage::Ready {
            keywords: vec!["玄女".into()],
        };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"ready""#));
        let back: ServerMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn server_wake_roundtrip() {
        let at: DateTime<Utc> = "2026-05-10T12:34:56Z".parse().unwrap();
        let m = ServerMessage::Wake {
            keyword: "玄女".into(),
            score: 0.85,
            at,
        };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"wake""#));
        assert!(s.contains(r#""keyword":"玄女""#));
        let back: ServerMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn server_ping_roundtrip() {
        let at: DateTime<Utc> = "2026-05-10T12:34:56Z".parse().unwrap();
        let m = ServerMessage::Ping { at };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"ping""#));
        let back: ServerMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn server_error_roundtrip() {
        let m = ServerMessage::Error {
            code: "unauthorized".into(),
            message: "token 不对".into(),
        };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"error""#));
        assert!(s.contains(r#""code":"unauthorized""#));
        let back: ServerMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn server_bye_roundtrip() {
        let m = ServerMessage::Bye;
        let s = serde_json::to_string(&m).expect("ser");
        assert_eq!(s, r#"{"type":"bye"}"#);
        let back: ServerMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn client_hello_roundtrip() {
        let m = ClientMessage::Hello {
            client: "jarvis-mac".into(),
            version: "0.1.0".into(),
        };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"hello""#));
        let back: ClientMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn client_pong_roundtrip() {
        let at: DateTime<Utc> = "2026-05-10T12:34:56Z".parse().unwrap();
        let m = ClientMessage::Pong { at };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"pong""#));
        let back: ClientMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn client_keywords_roundtrip() {
        let m = ClientMessage::Keywords {
            words: vec!["玄女".into(), "贾维斯".into()],
        };
        let s = serde_json::to_string(&m).expect("ser");
        assert!(s.contains(r#""type":"keywords""#));
        let back: ClientMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    #[test]
    fn client_bye_roundtrip() {
        let m = ClientMessage::Bye;
        let s = serde_json::to_string(&m).expect("ser");
        assert_eq!(s, r#"{"type":"bye"}"#);
        let back: ClientMessage = serde_json::from_str(&s).expect("de");
        assert_eq!(back, m);
    }

    /// 未知 type 必须返反序列化错——给 wire format 演化留一个明确报错点。
    #[test]
    fn unknown_type_fails() {
        let bad = r#"{"type":"unknown_xx","foo":"bar"}"#;
        assert!(serde_json::from_str::<ServerMessage>(bad).is_err());
        assert!(serde_json::from_str::<ClientMessage>(bad).is_err());
    }
}
