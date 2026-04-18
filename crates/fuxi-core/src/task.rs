//! Task type + state machine.
//!
//! The state machine is the contract between 玄女 (who dispatches) and
//! 门客 (who execute). Illegal transitions panic in debug, error in release.

use crate::id::{AgentId, TaskId};
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
        }
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
}
