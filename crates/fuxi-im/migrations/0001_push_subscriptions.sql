-- δ · Web Push 订阅表（Decision 14 E）。
--
-- WHY 列设计：
-- - id 用 UUID 文本主键，跟 fuxi 其它库一致。
-- - device_id 现在是软引用（无 FK 约束）——β 将在 0002_device_tokens.sql 单独
--   建 device_tokens 表，等两人 schema 都落盘后再考虑加 FK；先保活避免 schema-loader
--   双 owner 互依。NOT NULL 保留，未配对设备不允许订阅 push。
-- - endpoint UNIQUE：同一浏览器 Push Service 给同一 origin 只发一个 endpoint，
--   重复 subscribe 应当 upsert 而不是新增（用户清缓存后会换 endpoint，旧 endpoint
--   失效由 4xx push 响应清理，本表不做活性检查）。
-- - silence_until 由 `/api/push/silence` 写入；NULL 表示从不静音。

CREATE TABLE IF NOT EXISTS push_subscriptions (
    id            TEXT PRIMARY KEY,
    device_id     TEXT NOT NULL,
    endpoint      TEXT NOT NULL UNIQUE,
    p256dh        TEXT NOT NULL,
    auth          TEXT NOT NULL,
    silence_until TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_push_subs_device ON push_subscriptions(device_id);
