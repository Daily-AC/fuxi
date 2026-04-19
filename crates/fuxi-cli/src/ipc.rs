//! fuxi daemon ↔ subcommand 的 IPC 协议。
//!
//! 设计原则：
//! - **JSON 行协议**（一行一个 JSON）——人能 `nc -U` 手测，ops 友好
//! - **Unix socket**（`/tmp/fuxi.sock` 默认，env `FUXI_SOCK` 覆盖）——
//!   本机绑定，不过网不加 TLS 的烦恼
//! - **同步请求/响应**——client 发一条、读一条、断开；没有持久 session
//! - **事件流不走这条路**——那是 firehose Hub 的 WS/SSE 做的；IPC 只做命令
//!
//! v0.1 场景 spec §2.2 薄片 C 的承载。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 默认 socket 路径——`FUXI_SOCK` 环境变量可覆盖。
pub const DEFAULT_SOCK_ENV: &str = "FUXI_SOCK";

/// 决定本次使用的 socket 路径。
pub fn socket_path() -> PathBuf {
    std::env::var(DEFAULT_SOCK_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/fuxi.sock"))
}

/// 客户端发给 daemon 的命令。
///
/// 每条命令独立，daemon 回一条 [`Response`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// 起一个新门客。v0.1 只支持 cc，由 daemon 根据 `role` 查
    /// `skills/<role>/SKILL.md` 组装 profile。
    Spawn {
        role: String,
        /// 可选门客名（默认走 role-N）。
        name: Option<String>,
    },
    /// 给指定门客派个任务。
    Dispatch {
        agent_id: String,
        title: String,
        body: Option<String>,
    },
    /// 介入——向指定门客发话。
    Intervene {
        agent_id: String,
        mode: InterveneMode,
        text: String,
    },
    /// 查询门客状态。`agent_id=None` 返回全部概览。
    Status { agent_id: Option<String> },
    /// 列出所有门客。
    List,
    /// 杀指定门客（shutdown 它的 cc 进程）。
    Kill { agent_id: String },
    /// 关 daemon 本身。所有门客随之下线。
    Shutdown,
    /// 健康探活——daemon 回一条 `Pong`。
    Ping,
}

/// 介入模式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterveneMode {
    /// 追加式——当前 turn 结束后门客下一 turn 看到这条新消息。
    /// stdio/WS 都能做，是最稳的介入形态。
    Append,
    /// 打断式——当前 turn 立即中止，门客开始处理这条新话。
    /// 依赖 WS 模式（`control_request { subtype: "interrupt" }`）。
    Interrupt,
}

/// daemon 回给客户端的响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    /// 命令执行成功，payload 是 JSON 值（调用方按 cmd 解析）。
    Ok { data: serde_json::Value },
    /// 命令失败，`error` 是人类可读的原因。
    Err { error: String },
    /// `Ping` 的专属响应。
    Pong,
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        let value = serde_json::to_value(data).unwrap_or(serde_json::Value::Null);
        Self::Ok { data: value }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self::Err { error: msg.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_serialization_matches_tag_format() {
        let cmd = Command::Spawn {
            role: "dev".into(),
            name: None,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""cmd":"spawn""#), "got: {s}");
        assert!(s.contains(r#""role":"dev""#));

        let parsed: Command = serde_json::from_str(&s).unwrap();
        matches!(parsed, Command::Spawn { .. });
    }

    #[test]
    fn response_ok_and_err_roundtrip() {
        let ok = Response::ok(serde_json::json!({"id": "abc"}));
        let s = serde_json::to_string(&ok).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        match back {
            Response::Ok { data } => assert_eq!(data["id"], "abc"),
            other => panic!("expected Ok, got {other:?}"),
        }

        let err = Response::err("boom");
        let s = serde_json::to_string(&err).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        matches!(back, Response::Err { .. });
    }

    #[test]
    fn intervene_mode_uses_snake_case_wire() {
        let cmd = Command::Intervene {
            agent_id: "dev-1".into(),
            mode: InterveneMode::Interrupt,
            text: "stop".into(),
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""mode":"interrupt""#), "got: {s}");
    }

    #[test]
    fn socket_path_honors_env() {
        // unsafe 因为 set_var 在 std 2024 edition 是 unsafe——单线程 test 安全
        unsafe {
            std::env::set_var(DEFAULT_SOCK_ENV, "/tmp/fuxi-test-xyz.sock");
        }
        let p = socket_path();
        assert_eq!(p, PathBuf::from("/tmp/fuxi-test-xyz.sock"));
        unsafe {
            std::env::remove_var(DEFAULT_SOCK_ENV);
        }
    }
}
