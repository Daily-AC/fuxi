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
        /// 可选执行节点。`local` 表示强制本机；其他值（如 `home`）表示远端节点。
        node: Option<String>,
        /// 可选 CLI 覆写（`claude-code` / `codex`）。
        cli: Option<String>,
        /// P2 召回：把 `task-<id>` 在策府里的 session_id 装到
        /// `CcLaunchConfig.resume_session_id`。和 `recall_role` 互斥。
        recall_task: Option<String>,
        /// P2 召回：取该 role 最近活动的 session（subject=`role-<role>`,
        /// `query_one` 拿 updated_at DESC 最新一条）。
        recall_role: Option<String>,
        /// Decision 21 phase 1：项目 slug。指定后走 spawn_worker_in_project_sandbox
        /// 路径（per-门客 per-project L3 持久 sandbox），跨 task 复用。
        /// `None` = 走旧 generic agent-id worktree 路径（向后兼容）。
        /// serde default 让老 daemon 仍能解析新 wire（虽然 fuxi-cli 单进程同版本，
        /// 但 dist gateway 有跨版本交互）。
        #[serde(default)]
        project: Option<String>,
        /// Decision 21 phase 2：非空 = 走 L2 ephemeral 路径，task uuid 锚定
        /// `~/.fuxi/projects/<project>/ephemeral/<task>/`。必须跟 `project`
        /// 配套。serde default 兼容老 wire。
        #[serde(default)]
        ephemeral_task: Option<String>,
    },
    /// 给指定门客派个任务。
    Dispatch {
        agent_id: String,
        /// 复用父任务 id（可选）：同一个 task_id 可派给多个门客。
        task_id: Option<String>,
        title: String,
        body: Option<String>,
        /// β · #70 dist 路由 hint：钉到指定 node_id（如 `mac-local`）。
        /// `None` = 不钉。Fuxi::dispatch 决策树看到 `pinned_node.is_some()` 即走
        /// dist enqueue，不走本地 spawn。serde default 兼容老 daemon。
        #[serde(default)]
        pinned_node: Option<String>,
        /// β · #70 dist 路由 hint：能力 tag 集合（如 `["local","erp"]`）。
        /// 非空时走 dist enqueue，controller 按 tag 匹配 worker。serde default
        /// 兼容老 daemon。
        #[serde(default)]
        required_tags: Vec<String>,
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
    /// 列出所有 dist worker 节点（来自 controller 的 nodes 表）。
    /// `--watch` 等是 CLI 层的事，IPC 上一次请求一次响应。
    Nodes,
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
    /// Jarvis · 语音模式：玄女通过 `fuxi xuannv say` 让 daemon publish 一条
    /// `XuannvVoiceLine` 事件。daemon 端在 publish 时会注入
    /// `meta.agent = xuannv_id`——CLI 进程拿不到运行时 xuannv_id，必须 daemon 兜底，
    /// 否则 `/api/conv` WS（按 `meta.agent==xuannv` 过滤）不会透传给 macOS App。
    XuannvVoiceLine {
        text: String,
        /// Phase 3 情绪映射：可选；`None` 走默认 normal。
        #[serde(default)]
        emotion: Option<String>,
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
            Self::XuannvVoiceLine { text, emotion } => {
                EventKind::XuannvVoiceLine { text, emotion }
            }
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

/// Wire 类型：单个 dist worker 节点的快照。
///
/// **不直接暴露 `Instant`**——内部表示不可序列化、跨进程也无意义；
/// 折成相对时间 `*_ms_ago`，让 daemon 是唯一的"时钟权威"。client/TUI 拿到
/// 后只做展示，不需要再校时。
///
/// `status` 是预先算好的人类友好标签：daemon 端按 `last_seen_ms_ago > 60_000`
/// 判 `stale`（与 `sweep_stale` 默认 60s 阈值一致），`None` 则 `unknown`。
/// β 的 metrics、γ 的事件、δ 的 TUI 都直接 import 这个类型——schema 一处定义。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeSnapshot {
    pub node_id: String,
    pub tags: Vec<String>,
    pub max_concurrency: u32,
    /// `inflight.len()`——TUI 可不展开 inflight 列表也够用；下钻才看 `inflight`。
    pub inflight_count: usize,
    pub inflight: Vec<String>,
    /// 自上次 `register/heartbeat/pull/report` 以来的毫秒数。`None` = 从未见过
    /// （理论不会出现在 nodes 表里，留给老版兼容）。
    pub last_seen_ms_ago: Option<u64>,
    /// 自首次 register 以来的毫秒数；重连不会重置。
    pub registered_at_ms_ago: Option<u64>,
    /// `"alive"` / `"stale"` / `"unknown"`——daemon 端按 60s 阈值预判。
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_serialization_matches_tag_format() {
        let cmd = Command::Spawn {
            role: "dev".into(),
            name: None,
            node: None,
            cli: None,
            recall_task: None,
            recall_role: None,
            project: None,
            ephemeral_task: None,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""cmd":"spawn""#), "got: {s}");
        assert!(s.contains(r#""role":"dev""#));

        let parsed: Command = serde_json::from_str(&s).unwrap();
        matches!(parsed, Command::Spawn { .. });
    }

    /// Decision 21 phase 1：`project` 字段 roundtrip + serde(default) 兼容性。
    /// 老 wire（无 project key）反序列化必须不挂，回出来 project=None。
    #[test]
    fn spawn_command_serdes_project_flag() {
        // 1. 带 project 字段
        let cmd = Command::Spawn {
            role: "luban".into(),
            name: None,
            node: None,
            cli: None,
            recall_task: None,
            recall_role: None,
            project: Some("erp".into()),
            ephemeral_task: None,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""project":"erp""#), "got: {s}");
        let back: Command = serde_json::from_str(&s).unwrap();
        match back {
            Command::Spawn { project, .. } => assert_eq!(project.as_deref(), Some("erp")),
            _ => panic!("not Spawn"),
        }

        // 2. 老 wire 无 project key——serde(default) 兜底回 None
        let legacy = r#"{"cmd":"spawn","role":"luban","name":null,"node":null,"cli":null,"recall_task":null,"recall_role":null}"#;
        let back: Command = serde_json::from_str(legacy).unwrap();
        match back {
            Command::Spawn { project, .. } => {
                assert!(project.is_none(), "老 wire 应反序列化 project=None")
            }
            _ => panic!("not Spawn"),
        }
    }

    /// Decision 21 phase 3：`ephemeral_task` 字段 roundtrip + serde(default) 兼容性。
    #[test]
    fn spawn_command_serdes_ephemeral_task_flag() {
        let cmd = Command::Spawn {
            role: "luban".into(),
            name: None,
            node: None,
            cli: None,
            recall_task: None,
            recall_role: None,
            project: Some("erp".into()),
            ephemeral_task: Some("task-abc".into()),
        };
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""ephemeral_task":"task-abc""#), "got: {s}");
        let back: Command = serde_json::from_str(&s).unwrap();
        match back {
            Command::Spawn { ephemeral_task, .. } => {
                assert_eq!(ephemeral_task.as_deref(), Some("task-abc"))
            }
            _ => panic!("not Spawn"),
        }
        // 老 wire 无 ephemeral_task → 必须 default 回 None
        let legacy = r#"{"cmd":"spawn","role":"luban","name":null,"node":null,"cli":null,"recall_task":null,"recall_role":null,"project":"erp"}"#;
        let back: Command = serde_json::from_str(legacy).unwrap();
        match back {
            Command::Spawn { ephemeral_task, .. } => assert!(ephemeral_task.is_none()),
            _ => panic!("not Spawn"),
        }
    }

    /// P2 召回：`recall_task` / `recall_role` 字段必须能完整 roundtrip——daemon
    /// 端反序列化失败 = 召回功能整条链路死。
    #[test]
    fn spawn_command_serdes_recall_flags() {
        let cmd = Command::Spawn {
            role: "dev".into(),
            name: None,
            node: Some("home".into()),
            cli: Some("codex".into()),
            recall_task: Some("task-abc".into()),
            recall_role: None,
            project: None,
            ephemeral_task: None,
        };
        let s = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&s).unwrap();
        match back {
            Command::Spawn {
                node,
                cli,
                recall_task,
                recall_role,
                ..
            } => {
                assert_eq!(node.as_deref(), Some("home"));
                assert_eq!(cli.as_deref(), Some("codex"));
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
    fn nodes_command_serdes() {
        let cmd = Command::Nodes;
        let s = serde_json::to_string(&cmd).unwrap();
        assert!(s.contains(r#""cmd":"nodes""#), "got: {s}");
        let back: Command = serde_json::from_str(&s).unwrap();
        matches!(back, Command::Nodes);
    }

    /// `NodeSnapshot` 是跨 IPC 的 wire 合约——β/γ/δ 都按这个 schema 拿数据。
    /// 字段重命名 = 三方一起爆炸，所以这里把所有字段名 + 类型 round-trip 一遍。
    #[test]
    fn node_snapshot_wire_roundtrip() {
        let snap = NodeSnapshot {
            node_id: "home".into(),
            tags: vec!["cc".into(), "codex".into()],
            max_concurrency: 4,
            inflight_count: 2,
            inflight: vec!["job-1".into(), "job-2".into()],
            last_seen_ms_ago: Some(300),
            registered_at_ms_ago: Some(123_456),
            status: "alive".into(),
        };
        let s = serde_json::to_string(&snap).unwrap();
        // 字段名锁定——TUI 端按 key 取，不能漏字段
        assert!(s.contains(r#""node_id":"home""#), "got: {s}");
        assert!(s.contains(r#""inflight_count":2"#), "got: {s}");
        assert!(s.contains(r#""last_seen_ms_ago":300"#), "got: {s}");
        assert!(s.contains(r#""registered_at_ms_ago":123456"#), "got: {s}");
        assert!(s.contains(r#""status":"alive""#), "got: {s}");
        let back: NodeSnapshot = serde_json::from_str(&s).unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn node_snapshot_serializes_none_timestamps_explicitly() {
        // Some(None) 区分 unknown / 没数据——必须显式写 null，不能 skip。
        let snap = NodeSnapshot {
            node_id: "n".into(),
            tags: vec![],
            max_concurrency: 1,
            inflight_count: 0,
            inflight: vec![],
            last_seen_ms_ago: None,
            registered_at_ms_ago: None,
            status: "unknown".into(),
        };
        let s = serde_json::to_string(&snap).unwrap();
        assert!(s.contains(r#""last_seen_ms_ago":null"#), "got: {s}");
        assert!(s.contains(r#""registered_at_ms_ago":null"#), "got: {s}");
    }

    /// Jarvis · 语音模式：`XuannvVoiceLine` payload 序列化 tag + 转 EventKind 保真。
    /// （注意：daemon 注入 `meta.agent = xuannv_id` 不在 payload 责任里，那是 daemon
    /// publish 路径上的事，单元测在 daemon 测试或集成测试里覆盖。）
    #[test]
    fn xuannv_voice_line_payload_roundtrips() {
        let p = EventKindPayload::XuannvVoiceLine {
            text: "好的，已派给鲁班".into(),
            emotion: Some("happy".into()),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["type"], "xuannv_voice_line");
        assert_eq!(v["text"], "好的，已派给鲁班");
        assert_eq!(v["emotion"], "happy");
        let back: EventKindPayload = serde_json::from_value(v).unwrap();
        let kind = back.into_event_kind();
        match kind {
            fuxi_core::EventKind::XuannvVoiceLine { text, emotion } => {
                assert_eq!(text, "好的，已派给鲁班");
                assert_eq!(emotion.as_deref(), Some("happy"));
            }
            other => panic!("expect XuannvVoiceLine, got {other:?}"),
        }
    }

    /// Phase 3：老 wire（无 emotion 字段）反序列化保兼容。
    #[test]
    fn xuannv_voice_line_payload_legacy_without_emotion() {
        let raw = serde_json::json!({
            "type": "xuannv_voice_line",
            "text": "我在，你说。"
        });
        let p: EventKindPayload = serde_json::from_value(raw).unwrap();
        match p {
            EventKindPayload::XuannvVoiceLine { text, emotion } => {
                assert_eq!(text, "我在，你说。");
                assert!(emotion.is_none());
            }
            other => panic!("expect XuannvVoiceLine, got {other:?}"),
        }
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
