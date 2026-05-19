//! Web Push (VAPID) 通知子系统（Decision 14 E）。
//!
//! 多层组合：
//! - [`keypair`] —— VAPID PEM keypair 持久化在 `~/.fuxi/im_vapid.json`
//! - [`store`] —— `push_subscriptions` 表 CRUD + silence_until 抑制
//! - [`notify`] —— 给所有 active 订阅 fan-out 一条通知（Web Push）
//! - [`fcm`] —— FCM（Firebase）原生 Android 推送的**平行**通道：
//!   `fcm_tokens` 表 CRUD + service account OAuth2 + fan-out
//! - [`hooks`] —— 订阅 EventBus，玄女 idle / root task done 时**同时**触发
//!   Web Push 和 FCM
//!
//! 公理对应：
//! - #1 显式沟通——通知是 PWA / Android app 不在前台时占用户 attention 的唯一通路
//! - #3 真实时——hooks 走 EventBus subscribe，不轮询
//! - #5 SQLite 单一真相——订阅 / fcm_token / silence_until 全在 im.db

pub mod fcm;
pub mod hooks;
pub mod keypair;
pub mod notify;
pub mod store;

pub use fcm::{
    FcmCredentials, FcmNotifyOutcome, FcmPusher, FcmSendOutcome, HttpFcmSender, NoopFcmSender,
    notify_fcm,
};
pub use keypair::{VapidKeypair, default_keypair_path, generate_at, load, load_or_generate};
pub use notify::{NotifyOutcome, PushPayload, PushSender, notify};
pub use store::{PushSubscriptionRow, list_active, set_silence_until, upsert};
