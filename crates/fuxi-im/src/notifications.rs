//! `NotificationStore` —— PWA「通知」tab + Issue 工作流的存取层。
//!
//! 数据模型见 `migrations/0005_notifications.sql` 表注释 + `migrations/0006_*.sql`
//! 加的 issue 工作流字段（status / fix_refs / events）。
//!
//! ## 用法路径
//! - 玄女 Bash 跑 `fuxi bug report ...` → fuxi-cli 调 `insert(NewNotification {kind:"bug",...})`
//! - PWA `GET /api/notifications` → handler 调 `list(...)`，按 status filter
//! - PWA tap 「测过了 → 关闭」→ `POST /api/notifications/{id}/close` →
//!   `close(id, actor, note)` → status='closed' + 写 events
//! - Claude `fuxi issue link-fix` → `link_fix(id, fix_ref, actor)` →
//!   fix_refs 累加 + status 自动 → 'awaiting_test' + 写 events
//!
//! 反向依赖：CLI 直开 SQLite 而非走 HTTP——避免 cc subprocess 跟 PWA cookie auth
//! 打交道；SQLite WAL 多写并发安全（IM-write + CLI-write 不冲突）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{Error, Result};

/// 单条通知的全字段视图——CLI 写入 + handler 出库 + 前端反序列化共用。
///
/// `fix_refs` / `events` 是 0006 migration 加的 JSON 列；wire 层就解成数组让前端
/// 直接渲染。老前端不识别这两字段也无碍（serde_json 多余字段忽略）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    /// kind 特定 JSON（如 bug 的 stack/git_commit / handoff 的 path）。
    pub metadata: Option<String>,
    pub created_at: String,
    pub read_at: Option<String>,
    pub closed_at: Option<String>,
    /// issue 工作流状态：'open' / 'awaiting_test' / 'closed'。kind != "bug" 的也
    /// 有这字段，默认 'open'，关闭后 'closed'——非 bug kind 不走 awaiting_test。
    #[serde(default = "default_status")]
    pub status: String,
    /// Claude push 的 fix commit 列表（`fuxi issue link-fix` 累加）。
    #[serde(default)]
    pub fix_refs: Vec<FixRef>,
    /// 状态机审计日志（创建 / 状态转 / fix 关联 / 关闭 / 重开 等）。
    #[serde(default)]
    pub events: Vec<IssueEvent>,
}

fn default_status() -> String {
    "open".to_string()
}

/// issue 状态机三态——和 DB `status` text 列对齐的强类型化。
///
/// 转移规则在 [`NotificationStore::update_status`] 中执行；入库时按 [`Self::as_str`]
/// 序列化文本，读出来时按 [`Self::from_db`] 兜底（认不出的 → `Open`，留余地给
/// 未来加状态时老二进制不炸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Open,
    AwaitingTest,
    Closed,
}

impl IssueStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::AwaitingTest => "awaiting_test",
            Self::Closed => "closed",
        }
    }

    /// 容忍未知值——读出 unknown status 默认按 Open 处理而不是 panic。
    pub fn from_db(s: &str) -> Self {
        match s {
            "awaiting_test" => Self::AwaitingTest,
            "closed" => Self::Closed,
            _ => Self::Open,
        }
    }
}

/// fix commit 引用——Claude push 修复后挂到 issue 上，PWA 详情页可点跳 GitHub。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FixRef {
    pub commit_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub at: String,
}

/// issue 状态机审计日志单条。GitHub 评论的轻量替代——讨论本体在 IM 聊天里。
///
/// `actor` 当前规范：`"xuannv"` / `"user"` / `"claude"` / `"system"`。
/// `action` 当前规范：`"created"` / `"status_changed"` / `"fix_linked"` /
///                   `"closed"` / `"reopened"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueEvent {
    pub actor: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: String,
}

/// 写入新通知用的 builder——`id` / `created_at` 自动生成。
///
/// `actor` 决定 `created` event 的 actor 字段。CLI / 玄女 自报 = "xuannv"；
/// 系统注入 = "system"。空 None 默认 "system"。
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub metadata: Option<String>,
    pub actor: Option<String>,
}

impl NewNotification {
    pub fn bug(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            kind: "bug".into(),
            severity: "warn".into(),
            title: title.into(),
            body: body.into(),
            task_id: None,
            agent_id: None,
            metadata: None,
            actor: None,
        }
    }

    /// 为 created event 指定 actor（"xuannv" / "user" / "claude" / "system"）。
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// 限定 kind（"bug" / "review_request" / ...）；None = 全部
    pub kind: Option<String>,
    /// 限定 status（'open' / 'awaiting_test' / 'closed'）；None = 不按 status 过
    pub status: Option<IssueStatus>,
    /// 兼容老调用：true 时不排除 closed。优先级低于 status——指定了 status 时
    /// 按 status 过，include_closed 失效。
    pub include_closed: bool,
    /// 上限（默认 200）
    pub limit: Option<i64>,
}

#[derive(Clone)]
pub struct NotificationStore {
    pool: SqlitePool,
}

impl NotificationStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 写一条新通知；返回完整 Notification（含自动生成的 id/created_at + 一条
    /// `created` event）。
    pub async fn insert(&self, n: NewNotification) -> Result<Notification> {
        let id = uuid::Uuid::new_v4().to_string();
        let created = now_iso();
        let actor = n.actor.unwrap_or_else(|| "system".to_string());
        let initial_event = IssueEvent {
            actor: actor.clone(),
            action: "created".into(),
            from: None,
            to: Some(IssueStatus::Open.as_str().into()),
            note: None,
            at: created.clone(),
        };
        let events = vec![initial_event];
        let events_json = serde_json::to_string(&events)
            .map_err(|e| Error::Internal(format!("events serialize: {e}")))?;
        let fix_refs: Vec<FixRef> = Vec::new();
        let fix_refs_json = serde_json::to_string(&fix_refs)
            .map_err(|e| Error::Internal(format!("fix_refs serialize: {e}")))?;

        sqlx::query(
            "INSERT INTO notifications \
             (id, kind, severity, title, body, task_id, agent_id, metadata, created_at, status, fix_refs, events) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&n.kind)
        .bind(&n.severity)
        .bind(&n.title)
        .bind(&n.body)
        .bind(&n.task_id)
        .bind(&n.agent_id)
        .bind(&n.metadata)
        .bind(&created)
        .bind(IssueStatus::Open.as_str())
        .bind(&fix_refs_json)
        .bind(&events_json)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("notifications insert: {e}")))?;
        Ok(Notification {
            id,
            kind: n.kind,
            severity: n.severity,
            title: n.title,
            body: n.body,
            task_id: n.task_id,
            agent_id: n.agent_id,
            metadata: n.metadata,
            created_at: created,
            read_at: None,
            closed_at: None,
            status: IssueStatus::Open.as_str().into(),
            fix_refs,
            events,
        })
    }

    /// 列通知。默认按 created_at 降序。filter 优先级：
    /// `status` Some 时按它过；否则 `include_closed=false` 排除 closed（老语义）。
    pub async fn list(&self, filter: ListFilter) -> Result<Vec<Notification>> {
        let limit = filter.limit.unwrap_or(200);
        let mut sql = String::from(
            "SELECT id, kind, severity, title, body, task_id, agent_id, metadata, \
             created_at, read_at, closed_at, status, fix_refs, events \
             FROM notifications WHERE 1=1 ",
        );
        if let Some(s) = filter.status {
            sql.push_str(&format!("AND status = '{}' ", s.as_str()));
        } else if !filter.include_closed {
            sql.push_str("AND status != 'closed' ");
        }
        if filter.kind.is_some() {
            sql.push_str("AND kind = ? ");
        }
        sql.push_str("ORDER BY created_at DESC LIMIT ?");

        let mut q = sqlx::query_as::<_, NotificationRow>(&sql);
        if let Some(k) = &filter.kind {
            q = q.bind(k);
        }
        q = q.bind(limit);
        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("notifications list: {e}")))?;
        rows.into_iter().map(Notification::try_from).collect()
    }

    /// 拉单条 Notification；不存在返 None。
    pub async fn get(&self, id: &str) -> Result<Option<Notification>> {
        let row = sqlx::query_as::<_, NotificationRow>(
            "SELECT id, kind, severity, title, body, task_id, agent_id, metadata, \
             created_at, read_at, closed_at, status, fix_refs, events \
             FROM notifications WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("notifications get: {e}")))?;
        row.map(Notification::try_from).transpose()
    }

    /// 统计未读数（unread = read_at IS NULL AND closed_at IS NULL）。
    /// 前端 tab badge 红点用此值。
    pub async fn unread_count(&self) -> Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications WHERE read_at IS NULL AND closed_at IS NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("notifications unread_count: {e}")))?;
        Ok(n)
    }

    /// 把指定 id 标 read（红点清掉但仍在列表里）。
    /// 幂等——已 read 不更新 read_at（保留首次时间）。
    pub async fn mark_read(&self, id: &str) -> Result<()> {
        let now = now_iso();
        sqlx::query("UPDATE notifications SET read_at = ? WHERE id = ? AND read_at IS NULL")
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("notifications mark_read: {e}")))?;
        Ok(())
    }

    /// 关闭通知（status='closed' + closed_at set + 写 events）。已 closed noop。
    ///
    /// `actor`：标记是谁关的（"user" / "xuannv" / "claude"），写进 events。
    pub async fn close(&self, id: &str, actor: &str, note: Option<&str>) -> Result<()> {
        self.update_status(id, IssueStatus::Closed, actor, note)
            .await?;
        Ok(())
    }

    /// 重新打开（status='open' + closed_at 清空 + 写 events）。
    pub async fn reopen(&self, id: &str, actor: &str, note: Option<&str>) -> Result<()> {
        self.update_status(id, IssueStatus::Open, actor, note)
            .await?;
        Ok(())
    }

    /// 一键全部 mark read——PWA 进入「通知」tab 时调，红点清零。
    pub async fn mark_all_read(&self) -> Result<i64> {
        let now = now_iso();
        let result = sqlx::query(
            "UPDATE notifications SET read_at = ? WHERE read_at IS NULL AND closed_at IS NULL",
        )
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("notifications mark_all_read: {e}")))?;
        Ok(result.rows_affected() as i64)
    }

    /// 通用状态机转换。从 DB 拿当前状态、转到 new_status、append events、按需
    /// 同步 closed_at。返回新状态串方便 handler echo。
    pub async fn update_status(
        &self,
        id: &str,
        new_status: IssueStatus,
        actor: &str,
        note: Option<&str>,
    ) -> Result<String> {
        let current = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("notification {id} 不存在")))?;
        let from_status = IssueStatus::from_db(&current.status);
        if from_status == new_status {
            // 幂等 noop——但 actor 想留 note 也允许走一次（写 event）
            if note.is_none() {
                return Ok(new_status.as_str().into());
            }
        }
        let now = now_iso();
        let action = match new_status {
            IssueStatus::Closed => "closed",
            IssueStatus::Open if from_status == IssueStatus::Closed => "reopened",
            _ => "status_changed",
        };
        let mut events = current.events;
        events.push(IssueEvent {
            actor: actor.into(),
            action: action.into(),
            from: Some(from_status.as_str().into()),
            to: Some(new_status.as_str().into()),
            note: note.map(str::to_string),
            at: now.clone(),
        });
        let events_json = serde_json::to_string(&events)
            .map_err(|e| Error::Internal(format!("events serialize: {e}")))?;
        // closed_at 跟 status 联动：转 closed 设；离开 closed 清
        let closed_at: Option<String> = match new_status {
            IssueStatus::Closed => Some(now.clone()),
            _ => None,
        };
        sqlx::query(
            "UPDATE notifications SET status = ?, closed_at = ?, events = ?, \
             read_at = COALESCE(read_at, ?) WHERE id = ?",
        )
        .bind(new_status.as_str())
        .bind(&closed_at)
        .bind(&events_json)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("notifications update_status: {e}")))?;
        Ok(new_status.as_str().into())
    }

    /// 关联 fix commit。dedupe 按 commit_sha——重复 link 同一 sha 不增加。状态
    /// 自动转换：`open` → `awaiting_test`，`closed` 不变（重开请用 `reopen`）。
    /// 写 events `fix_linked`。
    pub async fn link_fix(&self, id: &str, fix: FixRef, actor: &str) -> Result<()> {
        let current = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("notification {id} 不存在")))?;
        let now = now_iso();
        let mut fix_refs = current.fix_refs;
        let dedupe = fix_refs.iter().any(|f| f.commit_sha == fix.commit_sha);
        if !dedupe {
            fix_refs.push(fix.clone());
        }
        let fix_refs_json = serde_json::to_string(&fix_refs)
            .map_err(|e| Error::Internal(format!("fix_refs serialize: {e}")))?;

        // 状态决策：原 open → awaiting_test；其余保留
        let from_status = IssueStatus::from_db(&current.status);
        let new_status = match from_status {
            IssueStatus::Open => IssueStatus::AwaitingTest,
            other => other,
        };
        let mut events = current.events;
        events.push(IssueEvent {
            actor: actor.into(),
            action: "fix_linked".into(),
            from: Some(from_status.as_str().into()),
            to: Some(new_status.as_str().into()),
            note: Some(format!(
                "{}{}",
                fix.commit_sha,
                fix.summary
                    .as_deref()
                    .map(|s| format!(" — {s}"))
                    .unwrap_or_default()
            )),
            at: now.clone(),
        });
        let events_json = serde_json::to_string(&events)
            .map_err(|e| Error::Internal(format!("events serialize: {e}")))?;

        sqlx::query("UPDATE notifications SET fix_refs = ?, status = ?, events = ? WHERE id = ?")
            .bind(&fix_refs_json)
            .bind(new_status.as_str())
            .bind(&events_json)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("notifications link_fix: {e}")))?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct NotificationRow {
    id: String,
    kind: String,
    severity: String,
    title: String,
    body: String,
    task_id: Option<String>,
    agent_id: Option<String>,
    metadata: Option<String>,
    created_at: String,
    read_at: Option<String>,
    closed_at: Option<String>,
    status: String,
    fix_refs: String,
    events: String,
}

impl TryFrom<NotificationRow> for Notification {
    type Error = Error;

    fn try_from(r: NotificationRow) -> Result<Self> {
        // fix_refs / events 是 0006 加的 NOT NULL DEFAULT '[]' 列；老库 + 老
        // metadata 都不会进这里。空串 fallback 到 [] 防极端 BUG。
        let fix_refs: Vec<FixRef> = if r.fix_refs.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&r.fix_refs)
                .map_err(|e| Error::Internal(format!("fix_refs deserialize id={} {e}", r.id)))?
        };
        let events: Vec<IssueEvent> = if r.events.is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(&r.events)
                .map_err(|e| Error::Internal(format!("events deserialize id={} {e}", r.id)))?
        };
        Ok(Self {
            id: r.id,
            kind: r.kind,
            severity: r.severity,
            title: r.title,
            body: r.body,
            task_id: r.task_id,
            agent_id: r.agent_id,
            metadata: r.metadata,
            created_at: r.created_at,
            read_at: r.read_at,
            closed_at: r.closed_at,
            status: r.status,
            fix_refs,
            events,
        })
    }
}

/// 一致计算 ISO 时间字符串——Utc 当下时间格式化成 ISO 毫秒。
/// CLI 直开 DB 写入时复用这个，跟 store.insert 算的一致。
pub fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// 校验 ISO 字符串可解析（CLI 端用来确认时间戳格式没漂）。测试用。
pub fn parse_iso(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::Internal(format!("notifications parse_iso: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use tempfile::tempdir;

    async fn make_store() -> (tempfile::TempDir, NotificationStore) {
        let dir = tempdir().expect("tmp");
        let pool = db::init_at(dir.path().join("im.db")).await.expect("init");
        (dir, NotificationStore::new(pool))
    }

    #[tokio::test]
    async fn insert_and_list_returns_open_only_by_default() {
        let (_dir, store) = make_store().await;
        let n1 = store
            .insert(NewNotification::bug(
                "玄女 idle latency 偶发",
                "10 min 才 dead，应 60s",
            ))
            .await
            .expect("insert n1");
        // 等 1ms 让两条 created_at 错开，避免 DESC tiebreak 抖动
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let _n2 = store
            .insert(NewNotification {
                kind: "review_request".into(),
                severity: "info".into(),
                title: "鲁班 commit X 等审".into(),
                body: "diff 见 task-abc".into(),
                task_id: Some("task-abc".into()),
                agent_id: Some("agent-xyz".into()),
                metadata: None,
                actor: None,
            })
            .await
            .expect("insert n2");

        let list = store.list(ListFilter::default()).await.expect("list");
        assert_eq!(list.len(), 2);
        // 默认按 created_at DESC，n2 在前
        assert_eq!(list[0].kind, "review_request");
        assert_eq!(list[1].id, n1.id);
        assert_eq!(list[1].kind, "bug");
        assert!(list.iter().all(|n| n.closed_at.is_none()));
    }

    #[tokio::test]
    async fn close_hides_from_default_list() {
        let (_dir, store) = make_store().await;
        let n1 = store.insert(NewNotification::bug("a", "x")).await.unwrap();
        store.close(&n1.id, "user", None).await.expect("close");
        // default list 不含已 closed
        let open = store.list(ListFilter::default()).await.unwrap();
        assert!(open.is_empty(), "closed 应从默认列表隐藏");
        // include_closed 能查到
        let all = store
            .list(ListFilter {
                include_closed: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].closed_at.is_some());
        assert_eq!(all[0].status, "closed");
    }

    #[tokio::test]
    async fn unread_count_excludes_read_and_closed() {
        let (_dir, store) = make_store().await;
        let n1 = store.insert(NewNotification::bug("a", "")).await.unwrap();
        let n2 = store.insert(NewNotification::bug("b", "")).await.unwrap();
        let _n3 = store.insert(NewNotification::bug("c", "")).await.unwrap();
        assert_eq!(store.unread_count().await.unwrap(), 3);
        store.mark_read(&n1.id).await.unwrap();
        assert_eq!(store.unread_count().await.unwrap(), 2);
        store.close(&n2.id, "user", None).await.unwrap();
        assert_eq!(store.unread_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn mark_all_read_zeros_unread_count() {
        let (_dir, store) = make_store().await;
        store.insert(NewNotification::bug("a", "")).await.unwrap();
        store.insert(NewNotification::bug("b", "")).await.unwrap();
        let n = store.mark_all_read().await.unwrap();
        assert_eq!(n, 2);
        assert_eq!(store.unread_count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn close_idempotent_does_not_overwrite_read_at() {
        let (_dir, store) = make_store().await;
        let n = store.insert(NewNotification::bug("a", "")).await.unwrap();
        store.mark_read(&n.id).await.unwrap();
        let after_read = store
            .list(ListFilter {
                include_closed: true,
                ..Default::default()
            })
            .await
            .unwrap();
        let read_ts = after_read[0].read_at.clone();
        assert!(read_ts.is_some());
        // 再 close —— read_at 不该被重置
        store.close(&n.id, "user", None).await.unwrap();
        let after_close = store
            .list(ListFilter {
                include_closed: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(after_close[0].read_at, read_ts, "close 不应重置 read_at");
    }

    #[tokio::test]
    async fn parse_iso_round_trip() {
        let s = now_iso();
        let _dt = parse_iso(&s).expect("parse");
    }

    // ─── issue 工作流测试（v1-session19）───

    #[tokio::test]
    async fn insert_records_created_event_with_actor() {
        let (_dir, store) = make_store().await;
        let n = store
            .insert(NewNotification::bug("t", "b").with_actor("xuannv"))
            .await
            .unwrap();
        assert_eq!(n.status, "open");
        assert_eq!(n.events.len(), 1);
        assert_eq!(n.events[0].actor, "xuannv");
        assert_eq!(n.events[0].action, "created");
        assert_eq!(n.events[0].to.as_deref(), Some("open"));
        assert!(n.fix_refs.is_empty());
    }

    #[tokio::test]
    async fn link_fix_transitions_open_to_awaiting_test_and_appends_ref() {
        let (_dir, store) = make_store().await;
        let n = store
            .insert(NewNotification::bug("t", "b").with_actor("xuannv"))
            .await
            .unwrap();
        let fix = FixRef {
            commit_sha: "abc1234".into(),
            branch: Some("fix/foo".into()),
            summary: Some("修了".into()),
            at: now_iso(),
        };
        store.link_fix(&n.id, fix.clone(), "claude").await.unwrap();
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "awaiting_test", "open + fix → awaiting_test");
        assert_eq!(after.fix_refs.len(), 1);
        assert_eq!(after.fix_refs[0].commit_sha, "abc1234");
        // events: created + fix_linked
        assert_eq!(after.events.len(), 2);
        assert_eq!(after.events[1].actor, "claude");
        assert_eq!(after.events[1].action, "fix_linked");
        assert_eq!(after.events[1].from.as_deref(), Some("open"));
        assert_eq!(after.events[1].to.as_deref(), Some("awaiting_test"));
    }

    #[tokio::test]
    async fn link_fix_dedupes_same_commit_sha() {
        let (_dir, store) = make_store().await;
        let n = store.insert(NewNotification::bug("t", "")).await.unwrap();
        let fix = FixRef {
            commit_sha: "deadbeef".into(),
            branch: None,
            summary: None,
            at: now_iso(),
        };
        store.link_fix(&n.id, fix.clone(), "claude").await.unwrap();
        store.link_fix(&n.id, fix.clone(), "claude").await.unwrap();
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.fix_refs.len(), 1, "重复 sha 应 dedupe");
    }

    #[tokio::test]
    async fn close_transitions_status_and_writes_event() {
        let (_dir, store) = make_store().await;
        let n = store.insert(NewNotification::bug("t", "")).await.unwrap();
        store.close(&n.id, "user", Some("测过了")).await.unwrap();
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "closed");
        assert!(after.closed_at.is_some());
        let last = after.events.last().unwrap();
        assert_eq!(last.actor, "user");
        assert_eq!(last.action, "closed");
        assert_eq!(last.note.as_deref(), Some("测过了"));
    }

    #[tokio::test]
    async fn reopen_clears_closed_at_and_writes_event() {
        let (_dir, store) = make_store().await;
        let n = store.insert(NewNotification::bug("t", "")).await.unwrap();
        store.close(&n.id, "user", None).await.unwrap();
        store.reopen(&n.id, "user", Some("还有问题")).await.unwrap();
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "open");
        assert!(after.closed_at.is_none());
        let last = after.events.last().unwrap();
        assert_eq!(last.action, "reopened");
        assert_eq!(last.from.as_deref(), Some("closed"));
        assert_eq!(last.to.as_deref(), Some("open"));
    }

    #[tokio::test]
    async fn list_filter_by_status_returns_only_match() {
        let (_dir, store) = make_store().await;
        let a = store.insert(NewNotification::bug("a", "")).await.unwrap();
        let b = store.insert(NewNotification::bug("b", "")).await.unwrap();
        let _c = store.insert(NewNotification::bug("c", "")).await.unwrap();
        let fix = FixRef {
            commit_sha: "x".into(),
            branch: None,
            summary: None,
            at: now_iso(),
        };
        store.link_fix(&b.id, fix, "claude").await.unwrap();
        store.close(&a.id, "user", None).await.unwrap();

        let open = store
            .list(ListFilter {
                status: Some(IssueStatus::Open),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].title, "c");

        let waiting = store
            .list(ListFilter {
                status: Some(IssueStatus::AwaitingTest),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].title, "b");

        let closed = store
            .list(ListFilter {
                status: Some(IssueStatus::Closed),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].title, "a");
    }

    #[tokio::test]
    async fn update_status_to_same_state_is_noop_unless_note_present() {
        let (_dir, store) = make_store().await;
        let n = store.insert(NewNotification::bug("t", "")).await.unwrap();
        store
            .update_status(&n.id, IssueStatus::Open, "user", None)
            .await
            .unwrap();
        let after = store.get(&n.id).await.unwrap().unwrap();
        // open → open + 无 note → 没新 event
        assert_eq!(after.events.len(), 1);
        // 带 note 时允许写 event（用户/玄女 想留个理由）
        store
            .update_status(&n.id, IssueStatus::Open, "xuannv", Some("先 hold 着"))
            .await
            .unwrap();
        let after2 = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after2.events.len(), 2);
    }

    #[tokio::test]
    async fn migration_back_compat_old_closed_rows_have_status_closed() {
        // 模拟老库：插入一行不带 status/fix_refs/events 字段（用 INSERT 直绕过 store）
        let (_dir, store) = make_store().await;
        let n = store.insert(NewNotification::bug("t", "")).await.unwrap();
        // 直接 SQL 把 status 清空 + 设 closed_at（模拟老 schema 升级前状态）
        sqlx::query("UPDATE notifications SET closed_at = ?, status = 'closed' WHERE id = ?")
            .bind(now_iso())
            .bind(&n.id)
            .execute(&store.pool)
            .await
            .unwrap();
        let after = store.get(&n.id).await.unwrap().unwrap();
        assert_eq!(after.status, "closed");
        // list default 不含 closed
        let list = store.list(ListFilter::default()).await.unwrap();
        assert!(list.is_empty());
    }
}
