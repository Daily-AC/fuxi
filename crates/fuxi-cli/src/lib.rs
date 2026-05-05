//! # fuxi-cli internal library
//!
//! 此 lib 是 fuxi-cli binary 的内部脚手架 + bench/test 复用层，**不是稳定公共 API**。
//! 外部 crate 不应依赖此 lib——任何函数 / 类型签名可能在任何 minor version 改动。
//! 内部消费者：`benches/run_baseline.rs` 拿 `bench_support::*`，bin 入口 `main.rs` 复用全部模块。
//!
//! ---
//!
//! 之所以从纯 bin 升 lib + bin：bench target 必须有 lib 才能 `cargo bench`，
//! `dist.rs::mod tests` 那套 `#[cfg(test)]` 套路对 bench 不适合（不能 release-mode
//! 编 + standalone 跑）。lib + bin 是标准 pattern（clap / cargo 自身都这样）。
//!
//! 所有子模块都 `pub`——bin 入口 `main.rs` 通过 `use fuxi_cli::{...};` 拿到。
//! 内部跨模块仍走 `crate::foo` 路径，因为 `crate` 解析到 lib root，与原先
//! 在 `main.rs` 直接 `mod` 的语义一致。

pub mod autocomplete;
pub mod banner;
pub mod bench_support;
pub mod click_registry;
pub mod client;
pub mod clipboard;
pub mod command_registry;
pub mod daemon;
pub mod demo;
pub mod dist;
pub mod dist_auth;
pub mod dist_auth_client;
pub mod dist_event_client;
pub mod dist_persistence;
pub mod draft_stash;
pub mod extractor_hook;
pub mod im;
pub mod im_dist;
pub mod insight_cmd;
pub mod insight_extractor_hook;
pub mod ipc;
pub mod l2_gc;
pub mod markdown;
pub mod memory_cmd;
pub mod note_cmd;
pub mod profile_cmd;
pub mod project_cmd;
pub mod prompt_history;
pub mod recall_sink;
pub mod repl;
pub mod session;
pub mod skill;
pub mod spinner;
pub mod subcommands;
pub mod theme;
pub mod toast;
pub mod up;
pub mod watch;
pub mod xuannv_bootstrap;
pub mod xuannv_cmd;
