//! handler 模块集合。
//!
//! 按 Decision 14 路由表分文件：β 接 auth、γ 接 conv/stream、δ 接 push。
//! 骨架阶段每个文件留必要 stub，让 router 能挂上不 404、并明确 501 标识
//! "等 owner 实装"。

pub mod auth;
pub mod conv;
pub mod dispatch;
pub mod health;
pub mod intervene;
pub mod push;
pub mod tasks;

// γ · WS 通用循环 + cursor 解析 + 事件流构造——`conv` / `tasks` 共用。
pub(crate) mod ws_common;
