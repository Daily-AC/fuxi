-- β · Task #17 · IM 层聊天记录存储（独立于 fuxi-events 的 task-indexed schema）。
--
-- 为什么 IM 层独立存而不复用 fuxi-events：
-- - events 是 task-indexed 审计日志，前端要的是 conversation-indexed 时间线
-- - 首屏快速预加载要拿到"最近 N 条玄女对话"，按 task_id 找散在各 task 历史里翻成本高
-- - 解耦：events schema 改不影响 IM 协议；conversation 概念是 IM 域专属
--
-- conversation = 一条聊天线（玄女主线 / 任务子线 / 将来可能的群聊）。
-- message = 这条线里的单条消息（用户说的、agent 答的、task 卡片、文件、错误）。

CREATE TABLE IF NOT EXISTS conversations (
    id              TEXT PRIMARY KEY,                     -- uuid
    scope           TEXT NOT NULL,                        -- "xuannv" 主线 | "task:<task_id>" 子线
    title           TEXT,                                 -- nullable，主线 NULL 显默认；task 线放 title
    created_at      TEXT NOT NULL,
    last_active_at  TEXT NOT NULL,
    message_count   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_conv_last_active ON conversations(last_active_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_conv_scope ON conversations(scope);

-- WHY message_count 冗余：列表视图要"最近的 N 个 conversation + 各自消息条数"，
-- 不在 messages 表 COUNT(*) 走索引扫；写时 +1 是 O(1)，读时 select 这一列零成本。
-- conv_store 写消息路径必须事务里同步 update 这两个 mirror 列。

CREATE TABLE IF NOT EXISTS messages (
    id               TEXT PRIMARY KEY,                    -- uuid
    conv_id          TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role             TEXT NOT NULL,                       -- "user" | "xuannv" | "luban" | ... | "system"
    agent_id         TEXT,                                -- nullable，只 agent 消息有
    kind             TEXT NOT NULL,                       -- "text" | "task_card" | "tool_call" | "file" | "error"
    content          TEXT NOT NULL,                       -- JSON; schema 看 kind
    attachments      TEXT,                                -- JSON array of file_id refs (nullable)
    source_event_id  TEXT,                                -- FK events.db (跨 db 无外键约束，纯 traceability)
    ts               TEXT NOT NULL                        -- ISO 8601
);
CREATE INDEX IF NOT EXISTS idx_msg_conv_ts ON messages(conv_id, ts);
CREATE INDEX IF NOT EXISTS idx_msg_source ON messages(source_event_id);
