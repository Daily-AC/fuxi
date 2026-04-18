//! A2A v1.0 的 wire 类型——`serde` 序列化即对应 JSON-RPC body。
//!
//! 命名、字段与 Google 的 A2A 规范对齐（agent card、task、message、
//! artifact、part 等）；枚举的 wire 形态为 kebab-case（`input-required`、
//! `file` 等），跟规范保持一致，便于跨语言互通。
//!
//! 字段默认值与 `Option<T>` 的选择：规范中允许省略的字段用 `Option<T>`
//! 且 `#[serde(skip_serializing_if = "Option::is_none")]`，避免在 wire
//! 上出现一堆 `null`；枚举 tag 一律显式指定，防止未来加变体时默默 break。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 对外的 Agent Card——A2A 的能力自描述文件。
///
/// 与 `fuxi_core::AgentCard` 不是同一个概念：后者是内部注册表记录，
/// 包含 `AgentId`、进程状态等 **不应暴露给外部** 的字段；前者是跟
/// 外部握手时使用的能力声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub version: String,
    /// 本 agent 的 A2A 端点 URL（POST JSON-RPC 到这里）。
    pub url: String,
    pub capabilities: AgentCapabilities,
    pub skills: Vec<AgentSkill>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub streaming: bool,
    #[serde(rename = "pushNotifications")]
    pub push_notifications: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub examples: Vec<String>,
    /// 规范字段名是 camelCase，Rust 侧用 snake_case。
    #[serde(rename = "inputModes", default)]
    pub input_modes: Vec<String>,
    #[serde(rename = "outputModes", default)]
    pub output_modes: Vec<String>,
}

/// 状态机 on wire——kebab-case。
///
/// `InputRequired` 映射到 `"input-required"`：把人工介入显式地暴露给对端
/// 编排器（玄女），让她能决策是否转交主对话权。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Canceled,
}

impl TaskState {
    /// 终态意味着不会再有任何事件，SSE 流应当随后关闭。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    User,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Part 是 A2A 消息的内容原子单位——文本、结构化 data、文件三选一。
///
/// 使用 `#[serde(tag = "type")]`：wire 上是 `{ "type": "text", ... }`，
/// 避开 `untagged` 那种 "反序列化优先级决定语义" 的坑。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Part {
    Text { text: String },
    Data { data: serde_json::Value },
    File { file: FileContent },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// base64 编码后的字节内容。与 `uri` 互斥——二选一。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parts: Vec<Part>,
    pub index: u32,
    pub append: bool,
    #[serde(rename = "lastChunk")]
    pub last_chunk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub history: Vec<Message>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendTaskRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// 返回 Task 时包含最近 N 条历史；None 表示不限制。
    #[serde(rename = "historyLength", skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u32>,
    /// 客户端可接受的输出模态（`text`、`image/png` 等）；空 = 不限制。
    #[serde(
        rename = "acceptedOutputModes",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub accepted_output_modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendTaskResponse {
    pub task: Task,
}

/// SSE 流中的状态变化事件——`final: true` 表示这是终态事件，
/// 客户端可以关闭流。
///
/// 字段 `final` 跟 Rust 保留字冲突：wire 侧保持 `final`，Rust 侧改名
/// `is_final`，避免使用 raw identifier 污染下游代码。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStatusUpdateEvent {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub status: TaskStatus,
    #[serde(rename = "final")]
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskArtifactUpdateEvent {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub artifact: Artifact,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 19, 12, 0, 0).unwrap()
    }

    fn roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let s = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(value, &back, "roundtrip mismatch: {s}");
    }

    #[test]
    fn task_state_wire_is_kebab_case() {
        assert_eq!(
            serde_json::to_value(TaskState::InputRequired).unwrap(),
            json!("input-required")
        );
        assert_eq!(
            serde_json::to_value(TaskState::Submitted).unwrap(),
            json!("submitted")
        );
        assert_eq!(
            serde_json::to_value(TaskState::Canceled).unwrap(),
            json!("canceled")
        );
        let back: TaskState = serde_json::from_value(json!("input-required")).unwrap();
        assert_eq!(back, TaskState::InputRequired);
    }

    #[test]
    fn agent_card_roundtrip() {
        let card = AgentCard {
            name: "echo".into(),
            description: "echoes".into(),
            version: "0.1.0".into(),
            url: "http://127.0.0.1:4100/a2a".into(),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
            },
            skills: vec![AgentSkill {
                id: "echo".into(),
                name: "Echo".into(),
                description: "repeats".into(),
                tags: vec!["demo".into()],
                examples: vec!["hi".into()],
                input_modes: vec!["text".into()],
                output_modes: vec!["text".into()],
            }],
        };
        roundtrip(&card);
    }

    #[test]
    fn part_tagged_union_roundtrip() {
        let text = Part::Text { text: "hi".into() };
        let data = Part::Data {
            data: json!({ "k": 1 }),
        };
        let file = Part::File {
            file: FileContent {
                name: "a.txt".into(),
                mime_type: "text/plain".into(),
                bytes: Some("aGk=".into()),
                uri: None,
            },
        };
        roundtrip(&text);
        roundtrip(&data);
        roundtrip(&file);
        let v = serde_json::to_value(&text).unwrap();
        assert_eq!(v["type"], json!("text"));
    }

    #[test]
    fn task_roundtrip() {
        let task = Task {
            id: "t-1".into(),
            session_id: Some("s-1".into()),
            status: TaskStatus {
                state: TaskState::Working,
                message: Some(Message {
                    role: Role::Agent,
                    parts: vec![Part::Text {
                        text: "working".into(),
                    }],
                    metadata: None,
                }),
                timestamp: ts(),
            },
            history: vec![Message {
                role: Role::User,
                parts: vec![Part::Text {
                    text: "do x".into(),
                }],
                metadata: Some(json!({ "trace": "abc" })),
            }],
            artifacts: vec![Artifact {
                name: Some("result".into()),
                description: None,
                parts: vec![Part::Text {
                    text: "done".into(),
                }],
                index: 0,
                append: false,
                last_chunk: true,
            }],
        };
        roundtrip(&task);
    }

    #[test]
    fn send_task_request_roundtrip() {
        let req = SendTaskRequest {
            task_id: "t-1".into(),
            session_id: None,
            message: Message {
                role: Role::User,
                parts: vec![Part::Text {
                    text: "hello".into(),
                }],
                metadata: None,
            },
            metadata: None,
            history_length: Some(10),
            accepted_output_modes: vec!["text".into()],
        };
        roundtrip(&req);
    }

    #[test]
    fn status_and_artifact_events_roundtrip() {
        let s = TaskStatusUpdateEvent {
            task_id: "t-1".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: ts(),
            },
            is_final: true,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["final"], json!(true));
        roundtrip(&s);

        let a = TaskArtifactUpdateEvent {
            task_id: "t-1".into(),
            artifact: Artifact {
                name: None,
                description: None,
                parts: vec![Part::Text {
                    text: "chunk".into(),
                }],
                index: 0,
                append: false,
                last_chunk: false,
            },
        };
        roundtrip(&a);
    }

    #[test]
    fn terminal_predicate() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Canceled.is_terminal());
        assert!(!TaskState::Working.is_terminal());
        assert!(!TaskState::InputRequired.is_terminal());
    }
}
