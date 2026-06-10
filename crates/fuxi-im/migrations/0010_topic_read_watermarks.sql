-- Phase 2 · topic 未读水位（spec 2026-06-11-玄女分身-phase2-路由-design.md §4.5）
--
-- 读状态与 topic meta 正交，独立表不 ALTER topics：将来多端/多用户水位扩展
-- 不动 topics 行。无水位行 = 视为全读（unread=0）——避免升级首日 general
-- 的全量历史被计成四位数未读。
CREATE TABLE IF NOT EXISTS topic_read_watermarks (
    topic_id     TEXT PRIMARY KEY,
    last_read_at TEXT NOT NULL          -- RFC3339，与 messages.ts 同口径比较
);
