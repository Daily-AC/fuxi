//! Task-scoped agent mailbox built on EventBus.
//!
//! This is deliberately an audit layer, not a side channel. Every message is a
//! typed event with `meta.task`, sender/recipient ids, and a stable message id.

use crate::error::Result;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_events::EventBus;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailboxMessageState {
    Queued,
    Delivered,
    Read,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxMessage {
    pub message_id: Uuid,
    pub task: TaskId,
    pub from: AgentId,
    pub to: AgentId,
    pub text: String,
    pub summary: Option<String>,
    pub state: MailboxMessageState,
}

pub fn queue_agent_message(
    bus: &EventBus,
    task: TaskId,
    from: AgentId,
    to: AgentId,
    text: impl Into<String>,
    summary: Option<String>,
) -> Result<Uuid> {
    let message_id = Uuid::new_v4();
    let mut meta = EventMeta::now();
    meta.task = Some(task);
    meta.agent = Some(from);
    bus.publish(Event {
        meta,
        kind: EventKind::AgentMessageQueued {
            message_id,
            from,
            to,
            text: text.into(),
            summary,
        },
    })?;
    Ok(message_id)
}

pub fn mark_agent_message_delivered(
    bus: &EventBus,
    task: TaskId,
    message_id: Uuid,
    from: AgentId,
    to: AgentId,
) -> Result<()> {
    publish_mailbox_state(
        bus,
        task,
        to,
        EventKind::AgentMessageDelivered {
            message_id,
            from,
            to,
        },
    )
}

pub fn mark_agent_message_read(
    bus: &EventBus,
    task: TaskId,
    message_id: Uuid,
    reader: AgentId,
) -> Result<()> {
    publish_mailbox_state(
        bus,
        task,
        reader,
        EventKind::AgentMessageRead { message_id, reader },
    )
}

pub fn mark_agent_message_failed(
    bus: &EventBus,
    task: TaskId,
    message_id: Uuid,
    from: AgentId,
    to: AgentId,
    error: impl Into<String>,
) -> Result<()> {
    publish_mailbox_state(
        bus,
        task,
        to,
        EventKind::AgentMessageFailed {
            message_id,
            from,
            to,
            error: error.into(),
        },
    )
}

fn publish_mailbox_state(
    bus: &EventBus,
    task: TaskId,
    agent: AgentId,
    kind: EventKind,
) -> Result<()> {
    let mut meta = EventMeta::now();
    meta.task = Some(task);
    meta.agent = Some(agent);
    bus.publish(Event { meta, kind })?;
    Ok(())
}

pub async fn mailbox_for_agent(
    bus: &EventBus,
    task: TaskId,
    agent: AgentId,
) -> Result<Vec<MailboxMessage>> {
    let history = bus.history_for_task(task).await?;
    Ok(fold_mailbox(history.iter(), agent))
}

pub fn fold_mailbox<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    agent: AgentId,
) -> Vec<MailboxMessage> {
    let mut messages: Vec<MailboxMessage> = Vec::new();
    for ev in events {
        match &ev.kind {
            EventKind::AgentMessageQueued {
                message_id,
                from,
                to,
                text,
                summary,
            } if *to == agent => {
                let Some(task) = ev.meta.task else { continue };
                messages.push(MailboxMessage {
                    message_id: *message_id,
                    task,
                    from: *from,
                    to: *to,
                    text: text.clone(),
                    summary: summary.clone(),
                    state: MailboxMessageState::Queued,
                });
            }
            EventKind::AgentMessageDelivered { message_id, .. } => {
                if let Some(msg) = messages.iter_mut().find(|m| m.message_id == *message_id) {
                    msg.state = MailboxMessageState::Delivered;
                }
            }
            EventKind::AgentMessageRead { message_id, reader } if *reader == agent => {
                if let Some(msg) = messages.iter_mut().find(|m| m.message_id == *message_id) {
                    msg.state = MailboxMessageState::Read;
                }
            }
            EventKind::AgentMessageFailed {
                message_id, error, ..
            } => {
                if let Some(msg) = messages.iter_mut().find(|m| m.message_id == *message_id) {
                    msg.state = MailboxMessageState::Failed(error.clone());
                }
            }
            _ => {}
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(task: TaskId, agent: AgentId, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        meta.agent = Some(agent);
        Event { meta, kind }
    }

    #[test]
    fn fold_mailbox_tracks_delivery_read_and_failed_state() {
        let task = TaskId::new();
        let from = AgentId::new();
        let to = AgentId::new();
        let other = AgentId::new();
        let read_id = Uuid::new_v4();
        let failed_id = Uuid::new_v4();
        let ignored_id = Uuid::new_v4();
        let events = vec![
            ev(
                task,
                from,
                EventKind::AgentMessageQueued {
                    message_id: read_id,
                    from,
                    to,
                    text: "review diff".into(),
                    summary: Some("review".into()),
                },
            ),
            ev(
                task,
                from,
                EventKind::AgentMessageQueued {
                    message_id: failed_id,
                    from,
                    to,
                    text: "rerun tests".into(),
                    summary: None,
                },
            ),
            ev(
                task,
                from,
                EventKind::AgentMessageQueued {
                    message_id: ignored_id,
                    from,
                    to: other,
                    text: "not yours".into(),
                    summary: None,
                },
            ),
            ev(
                task,
                to,
                EventKind::AgentMessageDelivered {
                    message_id: read_id,
                    from,
                    to,
                },
            ),
            ev(
                task,
                to,
                EventKind::AgentMessageRead {
                    message_id: read_id,
                    reader: to,
                },
            ),
            ev(
                task,
                to,
                EventKind::AgentMessageFailed {
                    message_id: failed_id,
                    from,
                    to,
                    error: "receiver gone".into(),
                },
            ),
        ];

        let got = fold_mailbox(&events, to);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].state, MailboxMessageState::Read);
        assert_eq!(got[0].summary.as_deref(), Some("review"));
        assert_eq!(
            got[1].state,
            MailboxMessageState::Failed("receiver gone".into())
        );
    }

    #[tokio::test]
    async fn queue_and_state_markers_roundtrip_through_eventbus() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let task = TaskId::new();
        let from = AgentId::new();
        let to = AgentId::new();

        let id = queue_agent_message(&bus, task, from, to, "ship it", Some("done".into()))
            .expect("queue");
        mark_agent_message_delivered(&bus, task, id, from, to).expect("delivered");
        mark_agent_message_read(&bus, task, id, to).expect("read");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let got = mailbox_for_agent(&bus, task, to).await.expect("mailbox");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message_id, id);
        assert_eq!(got[0].text, "ship it");
        assert_eq!(got[0].state, MailboxMessageState::Read);
    }
}
