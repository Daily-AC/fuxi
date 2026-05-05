-- 论文 Memory Transfer Learning (arXiv:2604.14004) Insight 层落地。
--
-- v1 hetu_patterns 字段 (role/task_type/pattern/outcome/confidence/promoted_to_skill)
-- 表达"门客这次干这类活的小经验"，但论文核心是 abstraction dictates transferability：
-- 低层 trace 误传给门客 → negative transfer。所以加 4 个字段精准追踪 insight 抽象度
-- 与来源：
--   - abstraction_score (REAL nullable)：LLM-as-judge 打分 0.0-1.0；< 0.6 拒收
--   - derived_from_task (TEXT nullable)：关联 task uuid，方便审计 / 回溯
--   - source (TEXT nullable，应用层默认 'manual')：'cangjie-auto' / 'manual' / ...
--   - valid_until (TEXT nullable)：supersede 用，跟 oracle_facts 同语义
--
-- task_type 仍 NOT NULL（SQLite ALTER 改 NOT NULL → NULL 不支持），应用层用空串
-- 约定 "task-agnostic insight"——`NewPattern::insight()` 默认就空串。
--
-- 为什么本文件**不**直接拼到 SCHEMA_SQL：
-- ALTER TABLE 重复跑撞 "duplicate column name"。lib.rs::migrate_hetu_to_v2() 用
-- Rust 容错调度，CREATE INDEX IF NOT EXISTS 这条仍然 idempotent。
-- 本 .sql 留作**文档**——schema 演化历史的一份签到。

-- 加 4 列（lib.rs 容错执行；重复跑 ignore duplicate column 错）：
ALTER TABLE hetu_patterns ADD COLUMN abstraction_score REAL;
ALTER TABLE hetu_patterns ADD COLUMN derived_from_task TEXT;
ALTER TABLE hetu_patterns ADD COLUMN source TEXT;
ALTER TABLE hetu_patterns ADD COLUMN valid_until TEXT;

-- 活行索引（valid_until IS NULL）— recent_for_role / list_active 用。
CREATE INDEX IF NOT EXISTS idx_hetu_role_active
    ON hetu_patterns(role) WHERE valid_until IS NULL;
