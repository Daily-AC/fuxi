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
        /// P2 召回：把 `task-<id>` 在策府里的 session_id 装到
        /// `CcLaunchConfig.resume_session_id`。和 `recall_role` 互斥。
        recall_task: Option<String>,
        /// P2 召回：取该 role 最近活动的 session（subject=`role-<role>`,
        /// `query_one` 拿 updated_at DESC 最新一条）。
        recall_role: Option<String>,
    },
    /// 给指定门客派个任务。
    Dispatch {
        agent_id: String,
        /// 复用父任务 id（可选）：同一个 task_id 可派给多个门客。
        task_id: Option<String>,
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
    /// 玄女请示用户前标记任务 Blocked——发 `task_blocked` 事件。
    BlockTask { task_id: String, reason: String },
    /// 用户授权通过后解锁任务——发 `task_resumed` 事件，input 可选附带用户的话。
    ResumeTask {
        task_id: String,
        input: Option<String>,
    },
    /// 让 daemon 往 EventBus 推一条事件。当前只接招贤流水线需要的几种变体，
    /// 避免把整个 EventKind 暴露成 wire 协议（那会把内部型号锁死到 IPC 合约里）。
    EmitEvent { kind: EventKindPayload },
    /// 关 daemon 本身。所有门客随之下线。
    Shutdown,
    /// 健康探活——daemon 回一条 `Pong`。
    Ping,

    // ── 更漏（scheduler / triggers） ──
    /// 登记 cron trigger。
    CronAdd {
        expr: String,
        intent: String,
        tz: Option<String>,
        session_id: Option<String>,
    },
    /// 登记一次性 trigger（RFC3339 绝对时间）。
    CronOnce {
        at: String,
        intent: String,
        session_id: Option<String>,
    },
    /// 登记 fs_watch trigger。
    CronWatch {
        path: String,
        intent: String,
        events: Vec<String>,
        session_id: Option<String>,
    },
    /// 登记 webhook trigger——返回 trigger_id 以便用户拼 URL。
    CronWebhook {
        intent: String,
        secret: Option<String>,
        session_id: Option<String>,
    },
    /// 列出所有 triggers。
    CronList,
    /// 手动 fire 一条 trigger。
    CronFire { id: String },
    /// 删 trigger。
    CronRemove { id: String },
}

/// 客户端可以请求 daemon 推的事件负载——故意受限的白名单。
///
/// 为什么不直接重用 `fuxi_core::EventKind`：
/// 1. 避免把 event 变体通过 IPC 暴露出去的向前兼容负担
/// 2. 招贤事件的 meta（agent/task）天然为 None，换一层轻型结构更清晰
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKindPayload {
    SkillStaged {
        role: String,
        template: String,
        path: String,
    },
    SkillApproved {
        role: String,
    },
    SkillRejected {
        role: String,
        reason: String,
    },
    SkillActivated {
        role: String,
    },
    NoRoleMatched {
        need: String,
    },
}

impl EventKindPayload {
    /// 转成 `fuxi_core::EventKind`——daemon 拿到后用这个 publish。
    pub fn into_event_kind(self) -> fuxi_core::EventKind {
        use fuxi_core::EventKind;
        match self {
            Self::SkillStaged {
                role,
                template,
                path,
            } => EventKind::SkillStaged {
                role,
                template,
                path,
            },
            Self::SkillApproved { role } => EventKind::SkillApproved { role },
            Self::SkillRejected { role, reason } => EventKind::SkillRejected { role, reason },
            Self::SkillActivated { role } => EventKind::SkillActivated { role },
            Self::NoRoleMatched { need } => EventKind::NoRoleMatched { need },
        }
    }
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
            recall_task: None,
            recall_role: None,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""cmd":"spawn""#), "got: {s}");
        assert!(s.contains(r#""role":"dev""#));

        let parsed: Command = serde_json::from_str(&s).unwrap();
        matches!(parsed, Command::Spawn { .. });
    }

    /// P2 召回：`recall_task` / `recall_role` 字段必须能完整 roundtrip——daemon
    /// 端反序列化失败 = 召回功能整条链路死。
    #[test]
    fn spawn_command_serdes_recall_flags() {
        let cmd = Command::Spawn {
            role: "dev".into(),
            name: None,
            recall_task: Some("task-abc".into()),
            recall_role: None,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&s).unwrap();
        match back {
            Command::Spawn {
                recall_task,
                recall_role,
                ..
            } => {
                assert_eq!(recall_task.as_deref(), Some("task-abc"));
                assert!(recall_role.is_none());
            }
            other => panic!("expected Spawn, got {other:?}"),
        }
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
