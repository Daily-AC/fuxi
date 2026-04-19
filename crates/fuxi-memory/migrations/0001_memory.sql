-- 伏羲策府基础 schema。
-- 公理 #5：SQLite 单一真相源（WAL + 只 ADD）。两张正表 + 一个 standalone FTS5。

-- ── 甲骨：玄女长期事实 ───────────────────────────────────────────
-- 只 ADD：overwrite 禁止；冲突用 `supersede`——老行 `valid_until = now`
-- 再 INSERT 新行。`valid_until IS NULL` = 现行有效。
CREATE TABLE IF NOT EXISTS oracle_facts (
    id          TEXT PRIMARY KEY,              -- uuid v4
    subject     TEXT NOT NULL,                 -- 主体（用户名 / 门客角色 / 项目 ...）
    predicate   TEXT NOT NULL,                 -- 属性（preference / session_id / role ...）
    object      TEXT NOT NULL,                 -- 值（任意文本；结构化自行 JSON 编码）
    source      TEXT NOT NULL,                 -- 来源标签（user / agent:<id> / extractor ...）
    confidence  REAL NOT NULL DEFAULT 0.5,     -- 置信度 [0.0, 1.0]
    created_at  TEXT NOT NULL,                 -- ISO 8601 UTC
    updated_at  TEXT NOT NULL,
    valid_until TEXT                           -- NULL = 仍生效；supersede 后填入那一刻
);

CREATE INDEX IF NOT EXISTS idx_oracle_subject    ON oracle_facts(subject);
CREATE INDEX IF NOT EXISTS idx_oracle_sp         ON oracle_facts(subject, predicate);
CREATE INDEX IF NOT EXISTS idx_oracle_valid      ON oracle_facts(valid_until);

-- ── FTS5 standalone 索引 ────────────────────────────────────────
-- 为什么 standalone + 手动维护（而非 content-external + 触发器）：
--   unicode61 默认会把一整串连续 CJK 视作一个 token——"九天玄女" 索引进一个
--   token，搜 "玄女" 无法命中。用 trigram 又对 2 字查询失效（trigram 需 ≥3 字）。
--   让上层 `insert`/`fts_search` 在 Rust 里把每个 CJK 字符前后塞空格再入表/
--   查询，就能用 unicode61 默认分词同时满足 "玄女" 和 "prefers" 两类场景。
-- 代价：两次 INSERT、两次 SELECT——几微秒量级，可以忽略。
CREATE VIRTUAL TABLE IF NOT EXISTS oracle_fts USING fts5(
    fact_id UNINDEXED,
    search_text,
    tokenize='unicode61'
);

-- ── 河图洛书：门客经验模式 ────────────────────────────────────────
CREATE TABLE IF NOT EXISTS hetu_patterns (
    id                 TEXT PRIMARY KEY,       -- uuid v4
    role               TEXT NOT NULL,          -- 门客角色 (luban / cangjie / ...)
    task_type          TEXT NOT NULL,          -- 任务类别（自由文本：refactor / research / ...）
    pattern            TEXT NOT NULL,          -- 学到的模式/规则（自然语言）
    outcome            TEXT NOT NULL,          -- 上次验证结果（success / failure / partial / ...）
    confidence         REAL NOT NULL DEFAULT 0.5,
    created_at         TEXT NOT NULL,
    promoted_to_skill  INTEGER NOT NULL DEFAULT 0  -- 0/1；1 = 已晋升为 skill example
);

CREATE INDEX IF NOT EXISTS idx_hetu_role_task ON hetu_patterns(role, task_type);
CREATE INDEX IF NOT EXISTS idx_hetu_promoted  ON hetu_patterns(promoted_to_skill);
