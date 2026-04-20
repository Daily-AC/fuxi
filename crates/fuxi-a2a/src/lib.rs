//! `fuxi-a2a` · Google **A2A v1.0** 协议的 Rust 实现（core subset）。
//!
//! 本 crate 提供三块内容：
//! 1. **wire types**（`wire` 模块）——与 A2A 规范对齐的结构体/枚举，
//!    负责跟外部 agent 在 JSON 上的互通。M3.4 前是 `types` 模块；改名让
//!    "对外 wire 视图"语义在 import 路径上就能看出（`fuxi_a2a::wire::AgentCard`）；
//! 2. **server**（`server` 模块）——基于 axum 的 JSON-RPC 端点，
//!    通过 `A2AService` trait 把业务逻辑委托给具体 agent 实现；
//! 3. **client**（`client` 模块）——基于 reqwest 的薄客户端，
//!    `A2ARouter` 在对外发起 A2A 调用时使用。
//!
//! 注意：`fuxi_core::AgentCard` 是 **内部注册表视图**（含 AgentId、状态等
//! 不应暴露给外部的字段），而本 crate 的 `wire::AgentCard` 是 **对外 wire 视图**。
//! 用 `From<fuxi_core::AgentCard> for wire::AgentCard` 在边界做转换，**禁止**
//! 在 wire 边界外手抄字段。

pub mod client;
pub mod error;
pub mod jsonrpc;
pub mod server;
pub mod sse;
pub mod wire;

pub use client::A2AClient;
pub use error::{Error, Result};
pub use server::{A2AService, router};
pub use sse::{ServerSentEvent, ServerSentEventPayload};
pub use wire::{
    AgentCapabilities, AgentCard, AgentSkill, Artifact, FileContent, Message, Part, Role,
    SendTaskRequest, SendTaskResponse, Task, TaskArtifactUpdateEvent, TaskState, TaskStatus,
    TaskStatusUpdateEvent,
};
