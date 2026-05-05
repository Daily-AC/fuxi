-- 论文 Memory Transfer Learning 中的 Summary 层：用户身份卡。
-- 跟 oracle_facts（事实）严格分流——oracle 是"用户喜欢冰美式"这种零碎事实，
-- user_profile 是"以琳，工程师，主管产品；偏好极简术语；爱直球反馈"这种凝练身份。
-- 写入只 ADD：冲突走 supersede（老行 valid_until = now，再 insert 新行）。
CREATE TABLE IF NOT EXISTS user_profile (
    id          TEXT PRIMARY KEY,           -- uuid v4
    key         TEXT NOT NULL,              -- 'identity' / 'tone' / 'preferences' / ...
    value       TEXT NOT NULL,              -- 自然语言摘要（任意文本）
    source      TEXT NOT NULL,              -- 'manual' / 'cangjie-auto' / 'extractor' / ...
    created_at  TEXT NOT NULL,              -- ISO 8601 UTC
    updated_at  TEXT NOT NULL,
    valid_until TEXT                        -- NULL = 现行；supersede 后填那一刻
);

-- partial index 只覆盖 valid_until IS NULL 的活行——同 key 现行只有一条时
-- 命中尤其快。get(key) / list_active() 都靠它。
CREATE INDEX IF NOT EXISTS idx_user_profile_key_active
    ON user_profile(key) WHERE valid_until IS NULL;
