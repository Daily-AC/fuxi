//! `NotificationStore` —— PWA「通知」tab 的存取层。
//!
//! 数据模型见 `migrations/0005_notifications.sql` 表注释。
//!
//! 用法路径：
//! - 玄女 Bash 跑 `fuxi bug report ...` → fuxi-cli 调 `insert(NewNotification {kind:"bug",...})`
//! - PWA `GET /api/notifications` → handler 调 `list_open(...)` 拉未关闭的
//! - PWA tap 关闭 → `POST /api/notifications/{id}/close` → handler 调 `close(id)`
//! - PWA 打开页面时所有未读 → `mark_all_read(...)`（红点清零）
//!
//! 反向依赖：CLI 直开 SQLite 而非走 HTTP——避免 cc subprocess 跟 PWA cookie auth
//! 打交道；SQLite WAL 多写并发安全（IM-write + CLI-write 不冲突）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{Error, Result};

/// 单条通知的全字段视图——CLI 写入 + handler 出库 + 前端反序列化共用。
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
}

/// 写入新通知用的 builder——`id` / `created_at` 自动生成。
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub metadata: Option<String>,
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
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// 限定 kind（"bug" / "review_request" / ...）；None = 全部
    pub kind: Option<String>,
    /// 包含已关闭的（默认 false 只列 open）
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

    /// 写一条新通知；返回完整 Notification（含自动生成的 id/created_at）。
    pub async fn insert(&self, n: NewNotification) -> Result<Notification> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let created = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        sqlx::query(
            "INSERT INTO notifications (id, kind, severity, title, body, task_id, agent_id, metadata, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        })
    }

    /// 列通知。默认按 created_at 降序，only open（除非 filter.include_closed=true）。
    pub async fn list(&self, filter: ListFilter) -> Result<Vec<Notification>> {
        let limit = filter.limit.unwrap_or(200);
        let mut sql = String::from(
            "SELECT id, kind, severity, title, body, task_id, agent_id, metadata, created_at, read_at, closed_at FROM notifications WHERE 1=1 ",
        );
        if !filter.include_closed {
            sql.push_str("AND closed_at IS NULL ");
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
        Ok(rows.into_iter().map(Into::into).collect())
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
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        sqlx::query("UPDATE notifications SET read_at = ? WHERE id = ? AND read_at IS NULL")
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("notifications mark_read: {e}")))?;
        Ok(())
    }

    /// 关闭通知（从列表 default 隐藏）。已 closed 不更新。
    pub async fn close(&self, id: &str) -> Result<()> {
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        sqlx::query("UPDATE notifications SET closed_at = ?, read_at = COALESCE(read_at, ?) WHERE id = ? AND closed_at IS NULL")
            .bind(&now)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("notifications close: {e}")))?;
        Ok(())
    }

    /// 一键全部 mark read——PWA 进入「通知」tab 时调，红点清零。
    pub async fn mark_all_read(&self) -> Result<i64> {
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let result = sqlx::query(
            "UPDATE notifications SET read_at = ? WHERE read_at IS NULL AND closed_at IS NULL",
        )
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("notifications mark_all_read: {e}")))?;
        Ok(result.rows_affected() as i64)
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
}

impl From<NotificationRow> for Notification {
    fn from(r: NotificationRow) -> Self {
        Self {
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
        }
    }
}

/// 一致计算 `created_at` 字符串——Utc 当下时间格式化成 ISO 毫秒。
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
        let _n2 = store
            .insert(NewNotification {
                kind: "review_request".into(),
                severity: "info".into(),
                title: "鲁班 commit X 等审".into(),
                body: "diff 见 task-abc".into(),
                task_id: Some("task-abc".into()),
                agent_id: Some("agent-xyz".into()),
                metadata: None,
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
        store.close(&n1.id).await.expect("close");
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
        store.close(&n2.id).await.unwrap();
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
        store.close(&n.id).await.unwrap();
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
}
