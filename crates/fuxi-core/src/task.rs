//! Task type + state machine.
//!
//! The state machine is the contract between 玄女 (who dispatches) and
//! 门客 (who execute). Illegal transitions panic in debug, error in release.

use crate::id::{AgentId, TaskId};
use crate::project::ProjectId;
use crate::topic::TopicId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    New,
    Ready,
    InProgress,
    AwaitingInput,
    Delivering,
    Done,
    Blocked,
    Cancelled,
}

impl TaskState {
    /// Legal state transitions. Central spec for the state machine.
    pub fn can_transition_to(self, to: TaskState) -> bool {
        use TaskState::*;
        matches!(
            (self, to),
            (New, Ready)
                | (New, Cancelled)
                | (Ready, InProgress)
                | (Ready, Cancelled)
                | (InProgress, AwaitingInput)
                | (InProgress, Delivering)
                | (InProgress, Blocked)
                | (InProgress, Cancelled)
                | (AwaitingInput, InProgress)
                | (AwaitingInput, Cancelled)
                | (Blocked, Ready)
                | (Blocked, Cancelled)
                | (Delivering, Done)
                | (Delivering, Blocked)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub state: TaskState,
    pub assigned_to: Option<AgentId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// β · #57 dispatch routing：worker 必须满足 `required_tags ⊆ worker.tags`
    /// 才能取此 task。空 Vec = 无 tag 要求（默认走本地 home spawn 或任意能力的
    /// dist worker，按 `Fuxi::dispatch` 决策树）。
    /// `#[serde(default)]` 让老 Task JSON（无该字段）反序列化得空 Vec。
    #[serde(default)]
    pub required_tags: Vec<String>,
    /// β · #57 dispatch routing：玄女在 PWA composer 用 `@<node_id>` 显式 pin
    /// 到特定 dist 节点（如 `mac-local`）。`Some(...)` 时 Fuxi::dispatch 直接
    /// 派给该节点的 dist worker，跳过 tag 匹配 + 跳过本地 spawn。
    #[serde(default)]
    pub pinned_node: Option<String>,
    /// v2 跨节点 sandbox：task 关联的 project slug。`Some(...)` 时
    /// `Fuxi::dispatch` 在没显式 pinned_node 的情况下，按
    /// `Project.host_nodes` 自动 pin 到最闲节点；worker 收到 job 时 spawn
    /// 进对应 sandbox（resolve_project_sandbox_cwd）。
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    /// Phase 1 topic 路由：本 task 由哪个 topic 发起。`Some(...)` 时玄女切回该
    /// topic 前 worker 事件不污染当前 prompt（SystemEventBridge filter）；切回
    /// 后通过 prelude 摘要补出该 topic 进行中任务的状态。`None` = legacy task /
    /// 不挂任何 topic（视作默认 [`TopicId::general()`]）。
    /// `#[serde(default)]` 保 v1 events.db 反序列化老 task JSON 不挂。
    #[serde(default)]
    pub topic_id: Option<TopicId>,
}

impl Task {
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: TaskId::new(),
            title: title.into(),
            description: description.into(),
            state: TaskState::New,
            assigned_to: None,
            created_at: now,
            updated_at: now,
            required_tags: Vec::new(),
            pinned_node: None,
            project_id: None,
            topic_id: None,
        }
    }

    /// β · #57 builder：声明本 task 必须路由到拥有这些 tag 的 worker。
    pub fn with_required_tags(mut self, tags: Vec<String>) -> Self {
        self.required_tags = tags;
        self
    }

    /// β · #57 builder：把本 task pin 到指定 dist 节点。
    pub fn with_pinned_node(mut self, node_id: impl Into<String>) -> Self {
        self.pinned_node = Some(node_id.into());
        self
    }

    /// v2 跨节点：声明本 task 关联到某 project slug。
    pub fn with_project_id(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self
    }

    /// Phase 1 topic：声明本 task 由哪个 topic 发起。
    pub fn with_topic_id(mut self, topic_id: TopicId) -> Self {
        self.topic_id = Some(topic_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_transitions_legal() {
        use TaskState::*;
        assert!(New.can_transition_to(Ready));
        assert!(Ready.can_transition_to(InProgress));
        assert!(InProgress.can_transition_to(Delivering));
        assert!(Delivering.can_transition_to(Done));
    }

    #[test]
    fn cannot_skip_states() {
        use TaskState::*;
        assert!(!New.can_transition_to(InProgress));
        assert!(!New.can_transition_to(Done));
        assert!(!Ready.can_transition_to(Done));
    }

    #[test]
    fn terminal_states_do_not_leak_back() {
        use TaskState::*;
        for terminal in [Done, Cancelled] {
            for any in [New, Ready, InProgress, AwaitingInput, Delivering, Blocked] {
                assert!(
                    !terminal.can_transition_to(any),
                    "terminal {terminal:?} should not reach {any:?}"
                );
            }
        }
    }

    #[test]
    fn blocked_can_only_recover_to_ready_or_cancel() {
        use TaskState::*;
        assert!(Blocked.can_transition_to(Ready));
        assert!(Blocked.can_transition_to(Cancelled));
        assert!(!Blocked.can_transition_to(InProgress));
        assert!(!Blocked.can_transition_to(Done));
    }

    /// v2 跨节点：task 必须能携带 project_id 让 dispatch 决定路由。
    #[test]
    fn task_with_project_id_round_trip() {
        use crate::project::ProjectId;
        let pid = ProjectId::new("demo-site").unwrap();
        let t = Task::new("frontend", "build login page").with_project_id(pid.clone());
        assert_eq!(t.project_id.as_ref(), Some(&pid));

        // serde roundtrip 保留 project_id
        let json = serde_json::to_string(&t).unwrap();
        let t2: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(t2.project_id.as_ref(), Some(&pid));
    }

    /// 老 Task JSON（无 project_id 字段）反序列化得 None，不能 fail——升级
    /// 兼容性硬要求。
    #[test]
    fn task_deserializes_legacy_without_project_id() {
        // 起一份新 task，序列化后手工删 project_id 字段模拟 v2 之前的 JSON。
        let modern = Task::new("x", "y");
        let mut value: serde_json::Value = serde_json::to_value(&modern).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("project_id");
        obj.remove("pinned_node");
        obj.remove("required_tags");
        let legacy = serde_json::to_string(&value).unwrap();

        let t: Task = serde_json::from_str(&legacy).expect("legacy task 应反序列化");
        assert!(t.project_id.is_none());
        assert!(t.required_tags.is_empty());
        assert!(t.pinned_node.is_none());
    }

    /// Phase 1 Topic：task 必须能携带 topic_id，让 SystemEventBridge filter。
    #[test]
    fn task_with_topic_id_round_trip() {
        use crate::topic::TopicId;
        let tid = TopicId::new();
        let t = Task::new("画头像", "用户要的萝莉斯头像").with_topic_id(tid);
        assert_eq!(t.topic_id, Some(tid));

        let json = serde_json::to_string(&t).unwrap();
        let t2: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(t2.topic_id, Some(tid));
    }

    /// 老 Task JSON（缺 topic_id 字段）反序列化得 None，老 events.db 升级兼容。
    #[test]
    fn task_deserializes_legacy_without_topic_id() {
        let modern = Task::new("x", "y");
        let mut value: serde_json::Value = serde_json::to_value(&modern).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("topic_id");
        let legacy = serde_json::to_string(&value).unwrap();

        let t: Task = serde_json::from_str(&legacy).expect("legacy task 应反序列化");
        assert!(t.topic_id.is_none());
    }
}
