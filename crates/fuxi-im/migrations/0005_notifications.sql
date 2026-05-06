-- 通知表（v1-session16）—— PWA「通知」tab 的数据源。
--
-- 设计 spec: bug 收集器 + 后续上下文交接消息 / deliverable 推送 / 门客审阅请求
-- 都进同一张表，按 kind 区分。前端按 closed_at IS NULL 过滤未关闭的，按
-- read_at 算红点 unread 数。
--
-- 列设计 WHY：
-- - id 用 UUID 文本主键（跟 events / im_db 既有约定一致）。
-- - kind 是 string 而非 enum——后续加 kind 不改 schema（"context_handoff" /
--   "review_request" 等）。前端按 kind 选不同图标/颜色。
-- - severity {info|warn|error}：bug 用 error/warn；review_request 用 info；
--   handoff_offer 用 info。
-- - task_id / agent_id 软引用——便于 PWA 点 notification 跳转到对应 task/agent
--   thread；NULL 表示该 notification 不绑实体（如 platform 级 bug）。
-- - metadata JSON：kind 特定字段塞这里，避免每加一种 kind 加列（git_commit /
--   stack_trace / handoff_path 等）。
-- - read_at NULL = 未读，NOT NULL = 已 mark read（红点不算入）。
-- - closed_at NULL = open，NOT NULL = 已关闭（列表默认隐藏）。
--
-- 索引：未关闭项按 created_at 降序是主路径——加 partial index 把它做快。

CREATE TABLE IF NOT EXISTS notifications (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    severity   TEXT NOT NULL DEFAULT 'info',
    title      TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    task_id    TEXT,
    agent_id   TEXT,
    metadata   TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    read_at    TEXT,
    closed_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_notifications_open
    ON notifications(closed_at, created_at DESC)
    WHERE closed_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_notifications_kind
    ON notifications(kind, created_at DESC);
