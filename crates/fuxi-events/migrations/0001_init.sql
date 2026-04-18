-- 伏羲 EventBus 审计日志基础表。
-- 设计公理 #5：SQLite 是单一真相源（WAL 模式 + append-only）。
-- 这里只做“写一次 / 读多次”的事件日志，不做 UPDATE/DELETE。

CREATE TABLE IF NOT EXISTS events (
    id            TEXT PRIMARY KEY,          -- uuid
    at            TEXT NOT NULL,             -- ISO 8601，用于时间游标
    session       TEXT,
    agent         TEXT,
    task          TEXT,
    kind_tag      TEXT NOT NULL,             -- serde "type" tag，用于过滤
    payload       TEXT NOT NULL,             -- 完整 Event 的 JSON
    -- 为什么用毫秒精度 + INTEGER 自增 rowid：datetime('now') 秒级粒度会在同秒内插入时
    -- 造成 ordering 无法稳定决断；rowid 作 tiebreaker 保证插入顺序可还原。
    recorded_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_events_at ON events(at);
CREATE INDEX IF NOT EXISTS idx_events_agent ON events(agent);
CREATE INDEX IF NOT EXISTS idx_events_task ON events(task);
