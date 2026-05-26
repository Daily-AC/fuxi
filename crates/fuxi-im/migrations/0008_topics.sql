-- Phase 1（handoff session19）· Topic 一等公民
--
-- 治痛点：玄女与用户多话题并行时 cc 进程单线性导致 context 污染。
-- 切 topic = fuxi 关掉当前 cc + 起新 cc 注 topic-scoped prelude；
-- 路由层让 worker 事件只进发起 topic，跨 topic 进 inbox。
--
-- 数据放 im.db 而非 events.db：topic 是用户视角概念（跟 conversations/messages
-- 同库便于 join）；events.db 只需 topic_id 字段做 filter 不需要 join。

CREATE TABLE IF NOT EXISTS topics (
    id              TEXT PRIMARY KEY,                     -- TopicId uuid（含 general 兜底）
    title           TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    last_active_at  TEXT NOT NULL,
    pinned          INTEGER NOT NULL DEFAULT 0,           -- BOOL 0/1（决策 3：第一版不暴露 pin）
    archived_at     TEXT                                  -- NULL = 活跃；归档 ≠ 删除
);
CREATE INDEX IF NOT EXISTS idx_topics_last_active ON topics(archived_at, last_active_at DESC);

-- 默认 "general" topic：所有 Phase 1 之前的 messages 归这里。
-- WHY 固定 UUID：跟 TopicId::general() 同值，确保 vocabulary 与 schema 对齐。
INSERT OR IGNORE INTO topics (id, title, created_at, last_active_at, pinned, archived_at)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'general',
    '2026-04-19T00:00:00Z',
    '2026-04-19T00:00:00Z',
    0,
    NULL
);

-- messages 表加 topic_id 列。NOT NULL DEFAULT general 让老消息自动归位。
ALTER TABLE messages ADD COLUMN topic_id TEXT NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001';
CREATE INDEX IF NOT EXISTS idx_msg_topic_ts ON messages(topic_id, ts);
