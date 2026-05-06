//! handler 模块集合。
//!
//! 按 Decision 14 路由表分文件：β 接 auth、γ 接 conv/stream、δ 接 push。
//! 骨架阶段每个文件留必要 stub，让 router 能挂上不 404、并明确 501 标识
//! "等 owner 实装"。

pub mod auth;
pub mod conv;
// Decision 22 phase 1：交付收件箱（list / download）
pub mod deliverables;
pub mod dispatch;
pub mod health;
pub mod intervene;
// β · #55 dist topology 节点 tab 数据源
pub mod nodes;
// v1-session16：「通知」tab 数据源（bug 收集器 / 系统通知 / handoff offer）
pub mod notifications;
// v1-session17 task #9 「更多」hub 三个新页：策府事实 / 角色卡 / 更漏 trigger
pub mod cron;
pub mod memory;
pub mod roles;
// Decision 21 phase 1：Project 注册表读视图
pub mod projects;
pub mod push;
// β · #56 本地 worker onboarding：主密码 → secret/token + 静态脚本端点
pub mod setup_worker;
pub mod tasks;
pub mod upload;
// β · #27 镜像 /api/conv 但按 agent_id 过滤——私聊页（重设计 #N5）数据源
pub mod workers;

// γ · WS 通用循环 + cursor 解析 + 事件流构造——`conv` / `tasks` 共用。
pub(crate) mod ws_common;
