-- 伏羲更漏 · 候簿 / 应期。
-- 公理 #5：SQLite 单一真相源。trigger_fires append-only，triggers 可更新（熔断/disable）。

CREATE TABLE IF NOT EXISTS triggers (
    id                   TEXT PRIMARY KEY,       -- trg_<uuid>
    kind                 TEXT NOT NULL,          -- 'cron' | 'once' | 'fs_watch' | 'webhook'
    spec                 TEXT NOT NULL,          -- JSON，内容依 kind 而异
    intent               TEXT NOT NULL,          -- 自然语言原句，给玄女
    session_id           TEXT,                   -- 玄女持久 session（fuxi-memory 给）；NULL=新起
    enabled              INTEGER NOT NULL DEFAULT 1,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    max_failures        INTEGER NOT NULL DEFAULT 5,
    last_fired_at       TEXT,                    -- 记最近一次 fire 的时间，cron 计算下一 tick 用
    created_at          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_triggers_kind_enabled ON triggers(kind, enabled);

CREATE TABLE IF NOT EXISTS trigger_fires (
    id          TEXT PRIMARY KEY,                -- fire_<uuid>
    trigger_id  TEXT NOT NULL,
    fired_at    TEXT NOT NULL,
    cause       TEXT NOT NULL,                   -- 'scheduled' | 'manual' | 'webhook' | 'fs'
    status      TEXT NOT NULL,                   -- 'dispatched' | 'skipped' | 'failed'
    error       TEXT,
    payload     TEXT,                            -- JSON: webhook body / fs event 等
    FOREIGN KEY (trigger_id) REFERENCES triggers(id)
);

CREATE INDEX IF NOT EXISTS idx_trigger_fires_trigger_id ON trigger_fires(trigger_id);
CREATE INDEX IF NOT EXISTS idx_trigger_fires_fired_at ON trigger_fires(fired_at);
