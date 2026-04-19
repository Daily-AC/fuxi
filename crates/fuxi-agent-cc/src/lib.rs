//! `fuxi-agent-cc` · Claude Code 门客适配器。
//!
//! 负责把 `claude` 的 headless `--print --input-format stream-json
//! --output-format stream-json` 模式包成一个 `fuxi_core::Agent`。
//!
//! 模块组织：
//! - `config`：启动参数（model/cwd/profile 注入点）；
//! - `spawn`：真正的子进程 fork/exec 边界；
//! - `parser`：一行 stream-json → 中间 `CcEvent` → 零或多条 `fuxi_core::Event`；
//! - `agent`：`CcAgent` 本体，实现 `Agent` trait。

pub mod agent;
pub mod config;
pub mod parser;
pub mod spawn;
pub mod ws_bridge;

pub use agent::{CcAgent, CcError};
pub use config::{CcLaunchConfig, DEFAULT_MODEL_ENV, resolve_default_model};
pub use parser::{CcEvent, TranslateState, parse_line, translate};
pub use spawn::{SpawnedCc, spawn_claude};
pub use ws_bridge::{WsChannel, WsError};
