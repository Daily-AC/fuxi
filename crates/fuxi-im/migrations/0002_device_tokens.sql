-- β · 设备 token 表（Decision 14 D）。
--
-- 与 push_subscriptions 同库（im.db）：都属 IM 域 OLTP 数据，跟 fuxi-events
-- 的 append-only 审计日志哲学正交。
--
-- WHY 列设计：
-- - token_id 是 token payload 里的 device_id（uuid）；既是主键也是验签后再二次
--   定位 device record 的钩子（用于 revoke）。
-- - hmac_secret 列保留——v1 全局共享 ~/.fuxi/im_hmac.key，本列恒等于全局 key
--   的 base64url。**预留** future per-device rotation：吊销某设备时换它的 secret
--   而不影响全局。当前 verify 路径只用全局 key，本列只做"曾经签发"留痕。
-- - revoked_at NULL = 未吊销。`/devices revoke <id>` 写入当前时刻。
-- - expires_at 是 token claims 里 expires_at 的 mirror；DB 这层做"该 token 的
--   存档"，verify 路径仍以 token claims 内的 expires_at 为准（避免 DB 时钟漂移）。

CREATE TABLE IF NOT EXISTS device_tokens (
    token_id      TEXT PRIMARY KEY,
    device_name   TEXT NOT NULL,
    hmac_secret   TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    revoked_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_device_tokens_expires_at ON device_tokens(expires_at);
CREATE INDEX IF NOT EXISTS idx_device_tokens_revoked_at ON device_tokens(revoked_at);
