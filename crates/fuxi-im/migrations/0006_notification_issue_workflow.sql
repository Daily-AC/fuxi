-- v1-session19 issue workflow（GitHub-issue 化）—— 给 bug 类 notification 加状态机
-- 字段，这样玄女上报 → Claude 拉取修复 → 用户/玄女 测试 → 关闭 链路有清晰
-- 状态承载，PWA 不再只能看 open/closed 二态。
--
-- 字段 WHY：
-- - status：3 态枚举 'open' / 'awaiting_test' / 'closed'。Claude push fix 自动转
--   awaiting_test，让 user 知道可以测了；测过了手动 closed。closed_at 保留作为
--   关闭时间戳（status=closed 必有 closed_at），既兼容老查询又承载语义。
-- - fix_refs：JSON 数组，每条 {commit_sha, branch, summary, at}。Claude
--   `fuxi issue link-fix` 时往里追加，PWA 详情页渲染成可点 commit 链接列表。
-- - events：JSON 数组，每条 {actor, action, from?, to?, note?, at}。状态机审计
--   日志（GitHub 评论的轻量替代——讨论本体在 IM 聊天里）。actor: "xuannv" /
--   "user" / "claude"，action: "created" / "status_changed" / "fix_linked" /
--   "closed" / "reopened"。
--
-- 老数据兼容：closed_at IS NOT NULL 的行 status='closed'，其余默认 'open'。
-- fix_refs/events 默认 '[]'（空 JSON 数组）让代码 deserialize 不炸。
--
-- status 索引：PWA 默认列只 open + awaiting_test（hide closed），加 partial index。

ALTER TABLE notifications ADD COLUMN status TEXT NOT NULL DEFAULT 'open';
ALTER TABLE notifications ADD COLUMN fix_refs TEXT NOT NULL DEFAULT '[]';
ALTER TABLE notifications ADD COLUMN events TEXT NOT NULL DEFAULT '[]';

-- 老 closed 行 status 同步上来，未来新代码靠 status 单字段过滤即可
UPDATE notifications SET status = 'closed' WHERE closed_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_notifications_status_kind
    ON notifications(status, kind, created_at DESC);
