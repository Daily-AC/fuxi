//! Phase 1 · Topic 存储层（im.db `topics` 表 CRUD）。
//!
//! Topic 是用户视角概念（"周末画画"/"研究 fuxi 架构"/...）。CRUD 落在 im.db 而非
//! events.db：跟 conversations/messages 同库便于 join；events.db 只通过
//! `EventMeta.topic_id` 做 filter 不需要 join。
//!
//! 与 [`crate::conv_store::ConvStore`] 平级，单独 file 是因为 topic CRUD 跟
//! 消息时间线两件事独立演化（topic 改 title / pin / archive 不动消息行）。

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use fuxi_core::{TopicId, TopicMeta};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// Topic CRUD 包装器——共享 [`ConvStore`] 同一个 SqlitePool。
#[derive(Clone)]
pub struct TopicStore {
    pool: SqlitePool,
}

impl TopicStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 创建一个新 topic（id 随机生成）并返回 meta。
    /// `created_at` / `last_active_at` 由本方法决定（now）。
    pub async fn create(&self, title: impl Into<String>) -> Result<TopicMeta> {
        let meta = TopicMeta::new(title);
        self.insert(&meta).await?;
        Ok(meta)
    }

    /// 显式插入一份 TopicMeta（譬如 migration 兜底插入 general / 测试预置）。
    /// 同 id 已存在则 noop（IGNORE），返回当前 db 里的版本。
    pub async fn insert(&self, meta: &TopicMeta) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO topics \
             (id, title, created_at, last_active_at, pinned, archived_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(meta.id.0.to_string())
        .bind(&meta.title)
        .bind(meta.created_at.to_rfc3339())
        .bind(meta.last_active_at.to_rfc3339())
        .bind(meta.pinned as i64)
        .bind(meta.archived_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("topic insert: {e}")))?;
        Ok(())
    }

    /// 拿单个 topic meta。不存在返 None。
    pub async fn get(&self, id: TopicId) -> Result<Option<TopicMeta>> {
        let row = sqlx::query(
            "SELECT id, title, created_at, last_active_at, pinned, archived_at \
             FROM topics WHERE id = ?1",
        )
        .bind(id.0.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("topic get: {e}")))?;
        row.map(row_to_topic).transpose()
    }

    /// 列出所有 topic，按 `last_active_at` 倒序。`include_archived=false` 时
    /// 只返活跃；true 时归档一并返。pinned 在 v1 不参与排序（决策 3）。
    pub async fn list(&self, include_archived: bool) -> Result<Vec<TopicMeta>> {
        let rows = if include_archived {
            sqlx::query(
                "SELECT id, title, created_at, last_active_at, pinned, archived_at \
                 FROM topics ORDER BY last_active_at DESC",
            )
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT id, title, created_at, last_active_at, pinned, archived_at \
                 FROM topics WHERE archived_at IS NULL ORDER BY last_active_at DESC",
            )
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| Error::Internal(format!("topic list: {e}")))?;
        rows.into_iter().map(row_to_topic).collect()
    }

    /// 把 topic 的 `last_active_at` 推到现在——switch_topic / 新消息入 topic 时调。
    /// 已归档 topic 也会被 touch（语义：归档 topic 重新被访问应"复活"前缀界面提示，
    /// v1 暂不实装复活，touch 不解归档；调用方按需自己 `unarchive`）。
    pub async fn touch_last_active(&self, id: TopicId) -> Result<()> {
        sqlx::query("UPDATE topics SET last_active_at = ?2 WHERE id = ?1")
            .bind(id.0.to_string())
            .bind(Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("topic touch: {e}")))?;
        Ok(())
    }

    /// 归档 topic。不删——消息保留，sidebar 默认不显（除非 list include_archived）。
    pub async fn archive(&self, id: TopicId) -> Result<()> {
        sqlx::query("UPDATE topics SET archived_at = ?2 WHERE id = ?1 AND archived_at IS NULL")
            .bind(id.0.to_string())
            .bind(Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("topic archive: {e}")))?;
        Ok(())
    }

    /// 解归档——把 `archived_at` 清零。v1 CLI 不暴露此接口，PWA 不暴露 pin/unpin
    /// （决策 3），方法保留给以后用。
    pub async fn unarchive(&self, id: TopicId) -> Result<()> {
        sqlx::query("UPDATE topics SET archived_at = NULL WHERE id = ?1")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("topic unarchive: {e}")))?;
        Ok(())
    }

    /// 改 title。归档 topic 也能改（用户决策 6：归档不删）。
    pub async fn rename(&self, id: TopicId, new_title: impl Into<String>) -> Result<()> {
        sqlx::query("UPDATE topics SET title = ?2 WHERE id = ?1")
            .bind(id.0.to_string())
            .bind(new_title.into())
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("topic rename: {e}")))?;
        Ok(())
    }

    /// 把 topic 的未读水位推到 now——PWA 进入/离开话题时调（POST /api/topics/:id/read）。
    pub async fn mark_read(&self, id: TopicId) -> Result<()> {
        sqlx::query(
            "INSERT INTO topic_read_watermarks (topic_id, last_read_at) VALUES (?1, ?2) \
             ON CONFLICT(topic_id) DO UPDATE SET last_read_at = excluded.last_read_at",
        )
        .bind(id.0.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("topic mark_read: {e}")))?;
        Ok(())
    }

    /// 每 topic 未读数：水位之后的非 user 消息（自己说的话不算未读）。
    /// 无水位行的 topic 不出现在结果里（= 0，视为全读）。
    /// ts 与水位同为 RFC3339 字符串比较——与 page_messages 的排序口径一致。
    pub async fn unread_counts(&self) -> Result<std::collections::HashMap<String, i64>> {
        let rows = sqlx::query(
            "SELECT m.topic_id AS topic_id, COUNT(*) AS n \
             FROM messages m \
             JOIN topic_read_watermarks w ON w.topic_id = m.topic_id \
             WHERE m.role != 'user' AND m.ts > w.last_read_at \
             GROUP BY m.topic_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("topic unread_counts: {e}")))?;
        rows.into_iter()
            .map(|row| {
                let topic_id: String = row
                    .try_get("topic_id")
                    .map_err(|e| Error::Internal(format!("unread row topic_id: {e}")))?;
                let n: i64 = row
                    .try_get("n")
                    .map_err(|e| Error::Internal(format!("unread row n: {e}")))?;
                Ok((topic_id, n))
            })
            .collect()
    }
}

fn row_to_topic(row: sqlx::sqlite::SqliteRow) -> Result<TopicMeta> {
    let id_str: String = row
        .try_get("id")
        .map_err(|e| Error::Internal(format!("topic row id: {e}")))?;
    let id = Uuid::parse_str(&id_str)
        .map_err(|e| Error::Internal(format!("topic uuid parse '{id_str}': {e}")))?;
    let title: String = row
        .try_get("title")
        .map_err(|e| Error::Internal(format!("topic row title: {e}")))?;
    let created_at: String = row
        .try_get("created_at")
        .map_err(|e| Error::Internal(format!("topic row created_at: {e}")))?;
    let last_active_at: String = row
        .try_get("last_active_at")
        .map_err(|e| Error::Internal(format!("topic row last_active_at: {e}")))?;
    let pinned_raw: i64 = row
        .try_get("pinned")
        .map_err(|e| Error::Internal(format!("topic row pinned: {e}")))?;
    let archived_at: Option<String> = row
        .try_get("archived_at")
        .map_err(|e| Error::Internal(format!("topic row archived_at: {e}")))?;

    Ok(TopicMeta {
        id: TopicId::from(id),
        title,
        created_at: parse_rfc3339(&created_at, "created_at")?,
        last_active_at: parse_rfc3339(&last_active_at, "last_active_at")?,
        pinned: pinned_raw != 0,
        archived_at: archived_at
            .as_deref()
            .map(|s| parse_rfc3339(s, "archived_at"))
            .transpose()?,
    })
}

fn parse_rfc3339(s: &str, field: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| Error::Internal(format!("topic {field} parse '{s}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conv_store::{ConvStore, SCOPE_XUANNV};
    use crate::db::init_at;

    async fn open_store() -> (tempfile::TempDir, TopicStore) {
        let dir = tempfile::tempdir().expect("tmp");
        let pool = init_at(&dir.path().join("im.db")).await.expect("init");
        let store = TopicStore::new(pool);
        (dir, store)
    }

    /// 往 topic 灌一条消息（role 可指定）——unread 口径测试用。
    async fn seed_msg(pool: &sqlx::SqlitePool, topic: TopicId, role: &str) {
        let conv = ConvStore::new(pool.clone());
        let conv_id = conv.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        conv.append_message_in_topic(
            &conv_id,
            role,
            None,
            "text",
            &serde_json::json!({"text": "msg"}),
            None,
            None,
            chrono::Utc::now(),
            &topic.0.to_string(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn no_watermark_means_zero_unread() {
        let (_dir, store) = open_store().await;
        let m = store.create("新话题").await.unwrap();
        seed_msg(&store.pool, m.id, "xuannv").await;
        let counts = store.unread_counts().await.unwrap();
        assert_eq!(counts.get(&m.id.0.to_string()), None, "无水位 = 全读");
    }

    #[tokio::test]
    async fn unread_counts_after_watermark_excludes_user_messages() {
        let (_dir, store) = open_store().await;
        let m = store.create("画画").await.unwrap();
        store.mark_read(m.id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        seed_msg(&store.pool, m.id, "xuannv").await;
        seed_msg(&store.pool, m.id, "user").await; // 自己说的话不算未读
        let counts = store.unread_counts().await.unwrap();
        assert_eq!(counts.get(&m.id.0.to_string()).copied(), Some(1));
    }

    #[tokio::test]
    async fn mark_read_clears_unread() {
        let (_dir, store) = open_store().await;
        let m = store.create("画画").await.unwrap();
        store.mark_read(m.id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        seed_msg(&store.pool, m.id, "xuannv").await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        store.mark_read(m.id).await.unwrap();
        let counts = store.unread_counts().await.unwrap();
        assert_eq!(counts.get(&m.id.0.to_string()), None, "read 后清零");
    }

    #[tokio::test]
    async fn migration_seeds_general_topic() {
        // 0008 migration 末尾 INSERT OR IGNORE 兜底插入 general。
        // 升级老 db 时也能拿到 general topic，老消息 DEFAULT general 才有去处。
        let (_dir, store) = open_store().await;
        let g = store
            .get(TopicId::general())
            .await
            .expect("get general")
            .expect("general topic 应被 migration 预置");
        assert_eq!(g.id, TopicId::general());
        assert_eq!(g.title, "general");
        assert!(!g.is_archived());
    }

    #[tokio::test]
    async fn create_then_get_round_trip() {
        let (_dir, store) = open_store().await;
        let m = store.create("周末画画").await.expect("create");
        let got = store
            .get(m.id)
            .await
            .expect("get")
            .expect("刚 create 的应 get 到");
        assert_eq!(got.id, m.id);
        assert_eq!(got.title, "周末画画");
        assert!(!got.is_archived());
        assert!(!got.pinned);
    }

    #[tokio::test]
    async fn list_excludes_archived_by_default() {
        let (_dir, store) = open_store().await;
        let a = store.create("活的").await.unwrap();
        let b = store.create("待归档").await.unwrap();
        store.archive(b.id).await.unwrap();

        let activ = store.list(false).await.expect("list active");
        let ids: Vec<TopicId> = activ.iter().map(|t| t.id).collect();
        assert!(ids.contains(&a.id), "活 topic 应在");
        assert!(!ids.contains(&b.id), "归档 topic 不应在 active list");
        assert!(ids.contains(&TopicId::general()), "general 总应在");

        let all = store.list(true).await.expect("list all");
        let all_ids: Vec<TopicId> = all.iter().map(|t| t.id).collect();
        assert!(all_ids.contains(&b.id), "include_archived 时归档应回来");
    }

    #[tokio::test]
    async fn touch_last_active_advances_timestamp() {
        let (_dir, store) = open_store().await;
        let m = store.create("画画").await.unwrap();
        let before = store.get(m.id).await.unwrap().unwrap().last_active_at;
        // touch 间隔 ≥ 1ms 让 rfc3339 微秒级不同
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        store.touch_last_active(m.id).await.unwrap();
        let after = store.get(m.id).await.unwrap().unwrap().last_active_at;
        assert!(after > before, "touch 后应推进 last_active_at");
    }

    #[tokio::test]
    async fn archive_is_idempotent_and_skips_archived() {
        let (_dir, store) = open_store().await;
        let m = store.create("结束的").await.unwrap();
        store.archive(m.id).await.unwrap();
        let first_archived = store.get(m.id).await.unwrap().unwrap().archived_at;
        assert!(first_archived.is_some());
        // 再 archive 一次：不变 (WHERE archived_at IS NULL 守护)
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        store.archive(m.id).await.unwrap();
        let second_archived = store.get(m.id).await.unwrap().unwrap().archived_at;
        assert_eq!(first_archived, second_archived, "二次 archive 不应改时间戳");
    }

    #[tokio::test]
    async fn rename_updates_title() {
        let (_dir, store) = open_store().await;
        let m = store.create("旧标题").await.unwrap();
        store.rename(m.id, "新标题").await.unwrap();
        let got = store.get(m.id).await.unwrap().unwrap();
        assert_eq!(got.title, "新标题");
    }

    #[tokio::test]
    async fn unarchive_clears_archived_at() {
        let (_dir, store) = open_store().await;
        let m = store.create("曾归档").await.unwrap();
        store.archive(m.id).await.unwrap();
        store.unarchive(m.id).await.unwrap();
        let got = store.get(m.id).await.unwrap().unwrap();
        assert!(got.archived_at.is_none(), "unarchive 应清空 archived_at");
    }
}
