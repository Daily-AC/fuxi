//! `fuxi-a2a` · Google **A2A v1.0** 协议的 Rust 实现（core subset）。
//!
//! 本 crate 提供三块内容：
//! 1. **wire types**（`types` 模块）——与 A2A 规范对齐的结构体/枚举，
//!    负责跟外部 agent 在 JSON 上的互通；
//! 2. **server**（`server` 模块）——基于 axum 的 JSON-RPC 端点，
//!    通过 `A2AService` trait 把业务逻辑委托给具体 agent 实现；
//! 3. **client**（`client` 模块）——基于 reqwest 的薄客户端，
//!    `A2ARouter` 在对外发起 A2A 调用时使用。
//!
//! 注意：`fuxi_core::AgentCard` 是 **内部注册表视图**，而本 crate 的
//! `types::AgentCard` 是 **对外 wire 视图**，两者语义不同，不得在类型层面
//! 合并。A2ARouter 负责在边界上互转。

pub mod client;
pub mod error;
pub mod jsonrpc;
pub mod server;
pub mod sse;
pub mod types;

pub use client::A2AClient;
pub use error::{Error, Result};
pub use server::{A2AService, router};
pub use sse::{ServerSentEvent, ServerSentEventPayload};
pub use types::{
    AgentCapabilities, AgentCard, AgentSkill, Artifact, FileContent, Message, Part, Role,
    SendTaskRequest, SendTaskResponse, Task, TaskArtifactUpdateEvent, TaskState, TaskStatus,
    TaskStatusUpdateEvent,
};
