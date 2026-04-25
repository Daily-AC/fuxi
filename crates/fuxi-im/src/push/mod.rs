//! Web Push (VAPID) 通知子系统（Decision 14 E）。
//!
//! 三层组合：
//! - [`keypair`] —— VAPID PEM keypair 持久化在 `~/.fuxi/im_vapid.json`
//! - [`store`] —— `push_subscriptions` 表 CRUD + silence_until 抑制
//! - [`notify`] —— 给所有 active 订阅 fan-out 一条通知
//! - [`hooks`] —— 订阅 EventBus，玄女 idle / root task done 触发 notify
//!
//! 公理对应：
//! - #1 显式沟通——通知是 PWA 不在前台时占用户 attention 的唯一通路
//! - #3 真实时——hooks 走 EventBus subscribe，不轮询
//! - #5 SQLite 单一真相——订阅/silence_until 全在 im.db

pub mod hooks;
pub mod keypair;
pub mod notify;
pub mod store;

pub use keypair::{VapidKeypair, default_keypair_path, generate_at, load, load_or_generate};
pub use notify::{NotifyOutcome, PushPayload, PushSender, notify};
pub use store::{PushSubscriptionRow, list_active, set_silence_until, upsert};
