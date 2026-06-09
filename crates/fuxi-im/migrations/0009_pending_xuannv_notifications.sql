-- 块 4 · 玄女分身完工持久队列（plan 2026-06-10-玄女分身-持久队列.md）
--
-- 治痛点：门客完工/求审时目标玄女分身已 dormant（休眠回收），bridge 抄送
-- 信号无活进程接收 → 信号丢失（a01cfab5 + 当日 zombie 场景）。
-- 把完工 prompt 落库排队，分身 respawn 时 drain 队列注入首 turn 补发。
--
-- 数据放 im.db：跟 topics / conversations 同库，topic 维度 join 方便；
-- 且补发是用户视角的「未读完工」概念，与 events.db append-only 审计正交。

CREATE TABLE IF NOT EXISTS pending_xuannv_notifications (
    id            TEXT PRIMARY KEY,
    topic_id      TEXT NOT NULL,
    prompt        TEXT NOT NULL,
    system_origin TEXT NOT NULL,   -- 来源标识（如门客 agent id / 事件 kind），便于审计去重
    created_at    TEXT NOT NULL,
    delivered_at  TEXT             -- NULL = 待补发；置值后 drain 不再取（幂等不重投）
);

-- drain_undelivered 按 (topic_id, delivered_at IS NULL) 过滤——索引覆盖此查询。
CREATE INDEX IF NOT EXISTS idx_pending_topic_undelivered
    ON pending_xuannv_notifications(topic_id, delivered_at);
