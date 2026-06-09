//! `PendingNotifyStore` —— 玄女分身完工补发的持久队列存取层。
//!
//! 数据模型见 `migrations/0009_pending_xuannv_notifications.sql`。
//!
//! ## 用法路径
//! - bridge 发现目标分身 dormant → `enqueue(topic, prompt, origin)` 落库 +
//!   触发 respawn（块 4.2）。
//! - 分身 respawn 后注首 turn 前 → `drain_undelivered(topic)` 取待补发条目，
//!   注入后 `mark_delivered(id)` 标记，避免下次 respawn 重投（块 5）。
//!
//! 走 im.db pool（与 NotificationStore / ConvStore 同库同 WAL）。

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::{Error, Result};
use crate::notifications::now_iso;

/// 一条待补发的完工通知。`delivered_at` 出库时一并带上方便调用方判重。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingNotification {
    pub id: String,
    pub topic_id: String,
    pub prompt: String,
    pub system_origin: String,
    pub created_at: String,
    pub delivered_at: Option<String>,
}

#[derive(Clone)]
pub struct PendingNotifyStore {
    pool: SqlitePool,
}

impl PendingNotifyStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 入队一条完工补发。`id` / `created_at` 自动生成；`delivered_at` 留 NULL。
    /// 返回生成的 id 方便调用方关联日志。
    pub async fn enqueue(
        &self,
        topic_id: &str,
        prompt: &str,
        system_origin: &str,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now_iso();
        sqlx::query(
            "INSERT INTO pending_xuannv_notifications \
             (id, topic_id, prompt, system_origin, created_at, delivered_at) \
             VALUES (?, ?, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(topic_id)
        .bind(prompt)
        .bind(system_origin)
        .bind(&created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("pending_notify enqueue: {e}")))?;
        Ok(id)
    }

    /// 取某 topic 下所有未补发条目，按 created_at 升序（先完工的先补）。
    /// 仅返回 `delivered_at IS NULL`——已补发的不再取（幂等不重投）。
    pub async fn drain_undelivered(&self, topic_id: &str) -> Result<Vec<PendingNotification>> {
        let rows = sqlx::query_as::<_, PendingRow>(
            "SELECT id, topic_id, prompt, system_origin, created_at, delivered_at \
             FROM pending_xuannv_notifications \
             WHERE topic_id = ? AND delivered_at IS NULL \
             ORDER BY created_at ASC",
        )
        .bind(topic_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("pending_notify drain: {e}")))?;
        Ok(rows.into_iter().map(PendingNotification::from).collect())
    }

    /// 标记某条已补发——置 delivered_at。幂等：已 delivered 不覆盖首次时间。
    pub async fn mark_delivered(&self, id: &str) -> Result<()> {
        let now = now_iso();
        sqlx::query(
            "UPDATE pending_xuannv_notifications SET delivered_at = ? \
             WHERE id = ? AND delivered_at IS NULL",
        )
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("pending_notify mark_delivered: {e}")))?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct PendingRow {
    id: String,
    topic_id: String,
    prompt: String,
    system_origin: String,
    created_at: String,
    delivered_at: Option<String>,
}

impl From<PendingRow> for PendingNotification {
    fn from(r: PendingRow) -> Self {
        Self {
            id: r.id,
            topic_id: r.topic_id,
            prompt: r.prompt,
            system_origin: r.system_origin,
            created_at: r.created_at,
            delivered_at: r.delivered_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use tempfile::tempdir;

    async fn make_store() -> (tempfile::TempDir, PendingNotifyStore) {
        let dir = tempdir().expect("tmp");
        let pool = db::init_at(dir.path().join("im.db")).await.expect("init");
        (dir, PendingNotifyStore::new(pool))
    }

    #[tokio::test]
    async fn enqueue_drain_returns_ascending_then_mark_delivered_empties() {
        let (_dir, store) = make_store().await;
        let topic = "topic-A";

        let id1 = store
            .enqueue(topic, "鲁班干完了 commit x", "agent-luban")
            .await
            .expect("enqueue 1");
        // 错开 created_at 让升序断言稳定（避免同毫秒 tiebreak 抖动）
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let id2 = store
            .enqueue(topic, "诸葛求审 commit y", "agent-zhuge")
            .await
            .expect("enqueue 2");

        // 另一 topic 的不该混进来
        store
            .enqueue("topic-B", "别的话题", "agent-other")
            .await
            .expect("enqueue other topic");

        let drained = store.drain_undelivered(topic).await.expect("drain");
        assert_eq!(drained.len(), 2, "topic-A 应有两条待补发");
        assert_eq!(drained[0].id, id1, "先入队的排前（created_at 升序）");
        assert_eq!(drained[1].id, id2);
        assert!(drained.iter().all(|p| p.delivered_at.is_none()));

        // 全部标记已补发后再 drain 应为空——幂等不重投
        store.mark_delivered(&id1).await.expect("mark 1");
        store.mark_delivered(&id2).await.expect("mark 2");
        let after = store.drain_undelivered(topic).await.expect("drain after");
        assert!(after.is_empty(), "已补发的不应再被 drain");

        // 重复 mark_delivered 不报错（幂等）
        store.mark_delivered(&id1).await.expect("mark 1 again");
    }
}
