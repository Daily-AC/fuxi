//! `fuxi-agent-codex` · Codex CLI 门客适配器。
//!
//! 把 OpenAI Codex CLI 的 `codex exec --json` 模式包成一个 `fuxi_core::Agent`。
//! 和 `fuxi-agent-cc` 是兄弟实现——**P2 允许少量并行代码**，P3 再抽共享 crate。
//!
//! 与 cc 的关键差异：
//! 1. **Codex exec 是 one-shot**：prompt 在启动时作为位置参数传入，**没有 stdin
//!    追写通道**。因此 `Agent::send_message` 在此适配器里直接返回错误。
//! 2. **事件 schema 不同**：codex 吐的是 `thread.started` / `turn.started` /
//!    `item.started` / `item.completed` / `turn.completed` / `error` /
//!    `turn.failed`，而不是 cc 的 `system` / `assistant` / `result`。
//! 3. **`--full-auto` 与 `--dangerously-bypass-approvals-and-sandbox` 互斥**——
//!    实测 codex-cli 0.118 报 `argument cannot be used with`。我们的配置把两者
//!    做成独立布尔，且 `build_args` 做互斥兜底（bypass 优先于 full_auto）。
//!
//! 模块：
//! - `config`：启动参数（model/cwd/full_auto/bypass/...）；
//! - `spawn`：真实子进程 fork/exec 边界；
//! - `parser`：一行 JSONL → 中间 `CodexEvent` → 零或多条 `fuxi_core::Event`；
//! - `agent`：`CodexAgent` 本体，实现 `Agent` trait。

pub mod agent;
pub mod config;
pub mod parser;
pub mod spawn;

pub use agent::{CodexAgent, CodexError};
pub use config::{
    CodexLaunchConfig, DEFAULT_MODEL_ENV, DEFAULT_MODEL_FALLBACK, resolve_default_model,
};
pub use parser::{CodexEvent, TranslateState, parse_line, translate};
pub use spawn::{SpawnedCodex, spawn_codex};
