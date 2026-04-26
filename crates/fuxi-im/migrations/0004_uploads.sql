-- β · Task #17 · 文件上传后端。
--
-- 设计取舍：
-- - sha256 主键不直接用——id 仍是 uuid，sha256 单独列+索引，因为同 hash 可能被
--   不同设备/不同时刻上传，每条记录追踪上传上下文，但磁盘只存一份（path 复用）
-- - path 在 ~/.fuxi/im_uploads/<sha[:2]>/<sha>.<ext>，两层散开避免单目录文件爆
-- - bytes 单独冗余列：列表展示时不读盘 stat
-- - mime 服务端校验后落库；客户端报的 mime 不可信
-- - owner_device 软引用 device_tokens.token_id（无外键约束因为可能跨 token 续命）

CREATE TABLE IF NOT EXISTS uploads (
    id            TEXT PRIMARY KEY,
    sha256        TEXT NOT NULL,
    name          TEXT,
    mime          TEXT,
    bytes         INTEGER NOT NULL,
    path          TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    owner_device  TEXT
);
CREATE INDEX IF NOT EXISTS idx_uploads_sha ON uploads(sha256);
CREATE INDEX IF NOT EXISTS idx_uploads_created ON uploads(created_at DESC);
