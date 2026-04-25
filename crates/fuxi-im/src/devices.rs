//! `device_tokens` 表 CRUD——配对成功后入库、TUI `/devices` 列出/吊销。
//!
//! 本模块不动 token 协议本身（见 [`crate::auth`]）；只管"哪些 token 被签发过、
//! 是否还活着"。verify 路径只用全局 HMAC key + claims 里的 `expires_at`；
//! 这里是配套的人审界面（`/devices revoke`）。

#![allow(dead_code)]

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::SqlitePool;

/// `device_tokens` 一行的内存视图——TUI `/devices` 渲染 + revoke 找目标用。
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    pub token_id: String,
    pub device_name: String,
    pub hmac_secret: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl DeviceRecord {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
}

/// 包装 SqlitePool 提供 device_tokens CRUD。多 handler clone 廉价。
#[derive(Clone)]
pub struct DeviceStore {
    pool: SqlitePool,
}

impl DeviceStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 插入一行。pair handler 签 token 成功后立即调用。
    ///
    /// 已存在同 token_id 时报错——device_id 是 uuid 应该不会撞，撞了说明 caller
    /// 复用了 uuid 这是 bug，宁可 fail-fast。
    pub async fn insert(&self, rec: &DeviceRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO device_tokens (token_id, device_name, hmac_secret, \
             created_at, expires_at, revoked_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&rec.token_id)
        .bind(&rec.device_name)
        .bind(&rec.hmac_secret)
        .bind(rec.created_at.to_rfc3339())
        .bind(rec.expires_at.to_rfc3339())
        .bind(rec.revoked_at.map(|t| t.to_rfc3339()))
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("device_tokens insert: {e}")))?;
        Ok(())
    }

    /// 列出全部（含已吊销）——TUI `/devices` 默认视图。按 `created_at` 升序。
    pub async fn list_all(&self) -> Result<Vec<DeviceRecord>> {
        let rows = sqlx::query(
            "SELECT token_id, device_name, hmac_secret, created_at, expires_at, revoked_at \
             FROM device_tokens ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("device_tokens list: {e}")))?;

        rows.into_iter().map(row_to_record).collect()
    }

    /// 按 token_id 拉单行；不存在返回 `Ok(None)`。
    pub async fn get(&self, token_id: &str) -> Result<Option<DeviceRecord>> {
        let row = sqlx::query(
            "SELECT token_id, device_name, hmac_secret, created_at, expires_at, revoked_at \
             FROM device_tokens WHERE token_id = ?1",
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("device_tokens get: {e}")))?;

        match row {
            Some(r) => Ok(Some(row_to_record(r)?)),
            None => Ok(None),
        }
    }

    /// 吊销：把 `revoked_at` 写为当前时刻。已吊销时不会重复写（idempotent）。
    /// 返 `Ok(true)` 表示真的更新了一行；`Ok(false)` 表示 token_id 不存在或已吊销。
    pub async fn revoke(&self, token_id: &str, now: DateTime<Utc>) -> Result<bool> {
        let res = sqlx::query(
            "UPDATE device_tokens SET revoked_at = ?2 \
             WHERE token_id = ?1 AND revoked_at IS NULL",
        )
        .bind(token_id)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("device_tokens revoke: {e}")))?;
        Ok(res.rows_affected() > 0)
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> Result<DeviceRecord> {
    let token_id: String = row
        .try_get("token_id")
        .map_err(|e| Error::Internal(format!("row token_id: {e}")))?;
    let device_name: String = row
        .try_get("device_name")
        .map_err(|e| Error::Internal(format!("row device_name: {e}")))?;
    let hmac_secret: String = row
        .try_get("hmac_secret")
        .map_err(|e| Error::Internal(format!("row hmac_secret: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| Error::Internal(format!("row created_at: {e}")))?;
    let expires_at_s: String = row
        .try_get("expires_at")
        .map_err(|e| Error::Internal(format!("row expires_at: {e}")))?;
    let revoked_at_s: Option<String> = row
        .try_get("revoked_at")
        .map_err(|e| Error::Internal(format!("row revoked_at: {e}")))?;

    let parse = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| Error::Internal(format!("rfc3339 parse '{s}': {e}")))
    };
    Ok(DeviceRecord {
        token_id,
        device_name,
        hmac_secret,
        created_at: parse(&created_at_s)?,
        expires_at: parse(&expires_at_s)?,
        revoked_at: revoked_at_s.as_deref().map(parse).transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use chrono::Duration as ChronoDuration;
    use tempfile::tempdir;

    async fn open_store() -> (tempfile::TempDir, DeviceStore) {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        (dir, DeviceStore::new(pool))
    }

    fn fixture(token_id: &str, name: &str) -> DeviceRecord {
        let now = Utc::now();
        DeviceRecord {
            token_id: token_id.to_string(),
            device_name: name.to_string(),
            hmac_secret: "k0".to_string(),
            created_at: now,
            expires_at: now + ChronoDuration::days(365),
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn insert_then_get_roundtrip() {
        let (_dir, store) = open_store().await;
        let rec = fixture("dev-1", "iPhone");
        store.insert(&rec).await.expect("insert");

        let got = store.get("dev-1").await.expect("get").expect("exists");
        assert_eq!(got.token_id, "dev-1");
        assert_eq!(got.device_name, "iPhone");
        assert!(got.revoked_at.is_none());
        // rfc3339 精度漂移容忍
        let drift = (got.created_at - rec.created_at).num_seconds().abs();
        assert!(drift <= 1);
    }

    #[tokio::test]
    async fn list_all_orders_by_created_at_ascending() {
        let (_dir, store) = open_store().await;
        let a = fixture("dev-a", "first");
        let mut b = fixture("dev-b", "second");
        // 拉开 created_at 的距离
        b.created_at = a.created_at + ChronoDuration::seconds(10);
        // expires_at 也对应跟移
        b.expires_at = a.expires_at + ChronoDuration::seconds(10);
        // 故意先插 b 后插 a，看排序是否仍按 created_at
        store.insert(&b).await.expect("insert b");
        store.insert(&a).await.expect("insert a");
        let rows = store.list_all().await.expect("list");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].token_id, "dev-a", "a 的 created_at 早，应排前");
        assert_eq!(rows[1].token_id, "dev-b");
    }

    #[tokio::test]
    async fn revoke_marks_revoked_at_and_is_idempotent() {
        let (_dir, store) = open_store().await;
        store.insert(&fixture("dev-1", "iPhone")).await.unwrap();
        let now = Utc::now();
        let first = store.revoke("dev-1", now).await.expect("first revoke");
        assert!(first, "首次 revoke 应返回 true");

        let got = store.get("dev-1").await.unwrap().unwrap();
        assert!(got.revoked_at.is_some(), "revoked_at 应已写入");

        // 再调一次——已吊销，应返回 false（不动 revoked_at）
        let again = store
            .revoke("dev-1", now + ChronoDuration::seconds(60))
            .await
            .unwrap();
        assert!(!again, "二次 revoke 应返回 false");
    }

    #[tokio::test]
    async fn revoke_unknown_token_returns_false() {
        let (_dir, store) = open_store().await;
        let res = store.revoke("never-existed", Utc::now()).await.unwrap();
        assert!(!res);
    }

    #[tokio::test]
    async fn is_active_reflects_revoked_and_expiry() {
        let now = Utc::now();
        let active = DeviceRecord {
            token_id: "x".into(),
            device_name: "x".into(),
            hmac_secret: "k".into(),
            created_at: now,
            expires_at: now + ChronoDuration::days(1),
            revoked_at: None,
        };
        assert!(active.is_active(now));

        let revoked = DeviceRecord {
            revoked_at: Some(now),
            ..active.clone()
        };
        assert!(!revoked.is_active(now));

        let expired = DeviceRecord {
            expires_at: now - ChronoDuration::seconds(1),
            ..active.clone()
        };
        assert!(!expired.is_active(now));
    }
}
