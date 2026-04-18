//! 端到端 A2A roundtrip：起一个最简 echo server，用 `A2AClient` 打全套调用。
//!
//! 目的是锁死 "wire + server + client" 三件套在一起跑通，尤其是 SSE
//! 流的帧切割——这部分只有跑真 HTTP 才能暴露 bug。

use async_trait::async_trait;
use chrono::Utc;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use fuxi_a2a::server::stream_from_events;
use fuxi_a2a::{
    A2AClient, A2AService, AgentCapabilities, AgentCard, AgentSkill, Artifact, Message, Part, Role,
    SendTaskRequest, ServerSentEvent, ServerSentEventPayload, Task, TaskState, TaskStatus,
};
use std::sync::{Arc, Mutex};

/// 最小可用的 echo agent：把收到的 user message 原样回成 artifact，
/// 期间推两条 SSE（working → completed）加一条 artifact。
struct EchoAgent {
    tasks: Mutex<Vec<Task>>,
}

impl EchoAgent {
    fn new() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl A2AService for EchoAgent {
    async fn agent_card(&self) -> fuxi_a2a::Result<AgentCard> {
        Ok(AgentCard {
            name: "echo".into(),
            description: "trivial echo agent for tests".into(),
            version: "0.1.0".into(),
            url: "http://127.0.0.1/a2a".into(),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
            },
            skills: vec![AgentSkill {
                id: "echo".into(),
                name: "Echo".into(),
                description: "echoes input".into(),
                tags: vec!["demo".into()],
                examples: vec!["hello".into()],
                input_modes: vec!["text".into()],
                output_modes: vec!["text".into()],
            }],
        })
    }

    async fn send_task(&self, req: SendTaskRequest) -> fuxi_a2a::Result<Task> {
        let task = Task {
            id: req.task_id.clone(),
            session_id: req.session_id.clone(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: Utc::now(),
            },
            history: vec![req.message.clone()],
            artifacts: vec![Artifact {
                name: Some("echo".into()),
                description: None,
                parts: req.message.parts.clone(),
                index: 0,
                append: false,
                last_chunk: true,
            }],
        };
        self.tasks.lock().unwrap().push(task.clone());
        Ok(task)
    }

    async fn send_task_subscribe(
        &self,
        req: SendTaskRequest,
    ) -> fuxi_a2a::Result<BoxStream<'static, ServerSentEvent>> {
        let id = req.task_id.clone();
        let working = fuxi_a2a::TaskStatusUpdateEvent {
            task_id: id.clone(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
        };
        let art = fuxi_a2a::TaskArtifactUpdateEvent {
            task_id: id.clone(),
            artifact: Artifact {
                name: Some("echo".into()),
                description: None,
                parts: req.message.parts.clone(),
                index: 0,
                append: false,
                last_chunk: true,
            },
        };
        let done = fuxi_a2a::TaskStatusUpdateEvent {
            task_id: id,
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: true,
        };
        Ok(stream_from_events(vec![
            ServerSentEvent::status(&working)?,
            ServerSentEvent::artifact(&art)?,
            ServerSentEvent::status(&done)?,
        ]))
    }

    async fn get_task(&self, id: &str) -> fuxi_a2a::Result<Task> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .ok_or_else(|| fuxi_a2a::Error::A2A(format!("no task {id}")))
    }

    async fn cancel_task(&self, id: &str) -> fuxi_a2a::Result<Task> {
        let mut guard = self.tasks.lock().unwrap();
        let t = guard
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| fuxi_a2a::Error::A2A(format!("no task {id}")))?;
        t.status = TaskStatus {
            state: TaskState::Canceled,
            message: None,
            timestamp: Utc::now(),
        };
        Ok(t.clone())
    }
}

async fn spawn_server() -> (String, tokio::task::JoinHandle<()>) {
    let service = Arc::new(EchoAgent::new());
    let router = fuxi_a2a::router(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}/a2a"), handle)
}

#[tokio::test]
async fn roundtrip_all_methods() {
    let (endpoint, _srv) = spawn_server().await;
    let client = A2AClient::new(&endpoint).unwrap();

    // agent/getCard
    let card = client.get_agent_card().await.unwrap();
    assert_eq!(card.name, "echo");
    assert!(card.capabilities.streaming);
    assert!(!card.capabilities.push_notifications);

    // tasks/send
    let req = SendTaskRequest {
        task_id: "t-1".into(),
        session_id: Some("s-1".into()),
        message: Message {
            role: Role::User,
            parts: vec![Part::Text {
                text: "hello world".into(),
            }],
            metadata: None,
        },
        metadata: None,
        history_length: None,
        accepted_output_modes: vec![],
    };
    let task = client.send_task(req.clone()).await.unwrap();
    assert_eq!(task.id, "t-1");
    assert_eq!(task.status.state, TaskState::Completed);
    assert_eq!(task.artifacts.len(), 1);

    // tasks/get
    let got = client.get_task("t-1").await.unwrap();
    assert_eq!(got.id, "t-1");

    // tasks/cancel
    let cancelled = client.cancel_task("t-1").await.unwrap();
    assert_eq!(cancelled.status.state, TaskState::Canceled);
}

#[tokio::test]
async fn roundtrip_subscribe_stream() {
    let (endpoint, _srv) = spawn_server().await;
    let client = A2AClient::new(&endpoint).unwrap();

    let req = SendTaskRequest {
        task_id: "t-42".into(),
        session_id: None,
        message: Message {
            role: Role::User,
            parts: vec![Part::Text {
                text: "stream please".into(),
            }],
            metadata: None,
        },
        metadata: None,
        history_length: None,
        accepted_output_modes: vec!["text".into()],
    };

    let mut stream = client.send_task_subscribe(req).await.unwrap();
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev.unwrap());
        if let Some(ServerSentEventPayload::Status(s)) = events.last()
            && s.is_final
        {
            break;
        }
    }

    assert_eq!(events.len(), 3, "expected working+artifact+completed");
    match &events[0] {
        ServerSentEventPayload::Status(s) => assert_eq!(s.status.state, TaskState::Working),
        _ => panic!("first event should be status/working"),
    }
    match &events[1] {
        ServerSentEventPayload::Artifact(a) => assert_eq!(a.artifact.index, 0),
        _ => panic!("second event should be artifact"),
    }
    match &events[2] {
        ServerSentEventPayload::Status(s) => {
            assert_eq!(s.status.state, TaskState::Completed);
            assert!(s.is_final);
        }
        _ => panic!("third event should be status/completed final"),
    }
}

#[tokio::test]
async fn unknown_method_returns_jsonrpc_error() {
    let (endpoint, _srv) = spawn_server().await;
    let http = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "nonsense/op",
        "params": {}
    });
    let resp = http.post(&endpoint).json(&body).send().await.unwrap();
    assert!(resp.status().is_success());
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["error"]["code"], serde_json::json!(-32601));
}
