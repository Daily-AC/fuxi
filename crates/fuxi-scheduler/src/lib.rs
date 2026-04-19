//! 伏羲更漏（fuxi-scheduler）· 定时 / 响应式 trigger 调度
//!
//! 两组件：
//! 1. **Keeper**（[`keeper`]）—— 1 s tick loop，驱动 cron / once trigger。
//! 2. **Watcher**（[`watcher`] / [`webhook`]）—— 响应式入口：fs / HTTP hook / manual fire。
//!
//! 两条路最终都把 [`fuxi_core::EventKind::TriggerFired`] 推进 EventBus，
//! 让 orchestrator 调度玄女。
//!
//! 公理：
//! - #3（真实时，不轮询）—— 响应式路径不轮询，cron tick 是调度器职责，不归下游。
//! - #4（CLI 工具层唯一）—— trigger CRUD 全部通过 `fuxi cron *` 子命令。
//! - #5（SQLite 单一真相源）—— `triggers` + `trigger_fires` 两表，append-only 历史。

pub mod error;
pub mod keeper;
pub mod prompt;
pub mod spec;
pub mod store;
pub mod watcher;
pub mod webhook;

pub use error::{Error, Result};
pub use keeper::{Clock, Keeper, KeeperConfig, SystemClock};
pub use prompt::build_trigger_prompt;
pub use spec::TriggerSpec;
pub use store::{FireCause, FireRecord, FireStatus, TriggerRow, TriggerStore};

/// Trigger id 前缀——`trg_<uuid>`；和 AgentId/TaskId 风格对齐。
pub fn new_trigger_id() -> String {
    format!("trg_{}", uuid::Uuid::new_v4())
}

/// fire_id 前缀。
pub fn new_fire_id() -> String {
    format!("fire_{}", uuid::Uuid::new_v4())
}
