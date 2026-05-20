-- FCM（Firebase Cloud Messaging）原生 Android 推送 token 表。
--
-- 与 push_subscriptions（Web Push / VAPID）平级、互不替代：FCM 是 Android
-- 原生 app 走的另一条推送通道，两路由 hooks 同时 fan-out（见 push/hooks.rs）。
--
-- WHY 列设计：
-- - token 是 FCM SDK 在客户端拿到的注册 token，做主键——同一 token 重复
--   register 走 upsert 而不是新增僵尸行（同 push_subscriptions 的 endpoint）。
-- - device_name 给用户调试可读（"以琳的 Pixel"），无 FK 软引用。
-- - token 失效（卸载 / 清数据）由 messages:send 返 404 + UNREGISTERED 时
--   notify_fcm 主动 delete_token 清理，本表不做活性探测。

CREATE TABLE IF NOT EXISTS fcm_tokens (
    token       TEXT PRIMARY KEY,
    device_name TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
