//! `push_subscriptions` 表操作。
//!
//! 三个核心动作：
//! - [`upsert`] —— 浏览器 `pushManager.subscribe()` 拿到的 (endpoint, keys) 入库；
//!   endpoint UNIQUE 决定重复订阅 = 更新现有行（而不是新增僵尸订阅）
//! - [`list_active`] —— `notify` fan-out 遍历，过滤当前 silence_until > now 的行
//! - [`set_silence_until`] —— `/api/push/silence` 写入；NULL 清除

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

/// 入库后的订阅条目（也是 list_active 的返回行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscriptionRow {
    pub id: String,
    pub device_id: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub silence_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// upsert 一条订阅——endpoint UNIQUE 决定 conflict 时更新 keys + 重置 silence_until。
///
/// 重置 silence_until 是 design choice：用户重新 subscribe 通常意味着设备状态变了
/// （清缓存 / 重装 PWA），把"当前禁推"语义带过来不合直觉。
pub async fn upsert(
    pool: &SqlitePool,
    device_id: &str,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
) -> Result<PushSubscriptionRow> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO push_subscriptions (id, device_id, endpoint, p256dh, auth, silence_until, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)
        ON CONFLICT(endpoint) DO UPDATE SET
            device_id = excluded.device_id,
            p256dh    = excluded.p256dh,
            auth      = excluded.auth,
            silence_until = NULL
        "#,
    )
    .bind(&id)
    .bind(device_id)
    .bind(endpoint)
    .bind(p256dh)
    .bind(auth)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await
    .map_err(|e| Error::Internal(format!("upsert push_subscription: {e}")))?;

    fetch_by_endpoint(pool, endpoint)
        .await?
        .ok_or_else(|| Error::Internal("upsert 后查不到订阅".to_string()))
}

/// 遍历当前可推送的订阅（silence_until 为 NULL 或已过）。
pub async fn list_active(pool: &SqlitePool) -> Result<Vec<PushSubscriptionRow>> {
    let now = Utc::now().to_rfc3339();
    let rows = sqlx::query_as::<_, RawRow>(
        r#"
        SELECT id, device_id, endpoint, p256dh, auth, silence_until, created_at
        FROM push_subscriptions
        WHERE silence_until IS NULL OR silence_until <= ?1
        ORDER BY created_at ASC
        "#,
    )
    .bind(&now)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Internal(format!("list_active: {e}")))?;

    rows.into_iter().map(into_row).collect()
}

/// 设置 silence_until。`None` = 立即清除静音。
///
/// `device_id` 而不是订阅 id：用户视角是"这台设备暂停 60s"，一台设备可能多个订阅
/// （生产里少见，但 PWA + 浏览器自身分别 subscribe 是可能的）。
pub async fn set_silence_until(
    pool: &SqlitePool,
    device_id: &str,
    until: Option<DateTime<Utc>>,
) -> Result<()> {
    let until_str = until.map(|t| t.to_rfc3339());
    sqlx::query("UPDATE push_subscriptions SET silence_until = ?1 WHERE device_id = ?2")
        .bind(until_str)
        .bind(device_id)
        .execute(pool)
        .await
        .map_err(|e| Error::Internal(format!("set_silence_until: {e}")))?;
    Ok(())
}

async fn fetch_by_endpoint(
    pool: &SqlitePool,
    endpoint: &str,
) -> Result<Option<PushSubscriptionRow>> {
    let row = sqlx::query_as::<_, RawRow>(
        r#"
        SELECT id, device_id, endpoint, p256dh, auth, silence_until, created_at
        FROM push_subscriptions WHERE endpoint = ?1
        "#,
    )
    .bind(endpoint)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Internal(format!("fetch_by_endpoint: {e}")))?;
    row.map(into_row).transpose()
}

#[derive(sqlx::FromRow)]
struct RawRow {
    id: String,
    device_id: String,
    endpoint: String,
    p256dh: String,
    auth: String,
    silence_until: Option<String>,
    created_at: String,
}

fn into_row(r: RawRow) -> Result<PushSubscriptionRow> {
    Ok(PushSubscriptionRow {
        id: r.id,
        device_id: r.device_id,
        endpoint: r.endpoint,
        p256dh: r.p256dh,
        auth: r.auth,
        silence_until: r.silence_until.map(|s| parse_rfc3339(&s)).transpose()?,
        created_at: parse_rfc3339(&r.created_at)?,
    })
}

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| Error::Internal(format!("parse rfc3339 {s}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use tempfile::tempdir;

    async fn fresh_pool() -> SqlitePool {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        // 让 dir 活到 pool drop 之后——返回时 dir 会 drop 文件还在
        std::mem::forget(dir);
        pool
    }

    /// upsert 新订阅 + list_active 看到。
    #[tokio::test]
    async fn upsert_then_list_active() {
        let pool = fresh_pool().await;
        let row = upsert(&pool, "dev-1", "https://push.example/abc", "p256", "auth")
            .await
            .expect("upsert");
        assert_eq!(row.device_id, "dev-1");
        assert!(row.silence_until.is_none());

        let active = list_active(&pool).await.expect("list");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].endpoint, "https://push.example/abc");
    }

    /// 同 endpoint 重复 upsert = 更新而不是新增。
    #[tokio::test]
    async fn upsert_idempotent_on_endpoint() {
        let pool = fresh_pool().await;
        let r1 = upsert(&pool, "dev-1", "https://e/a", "k1", "a1")
            .await
            .expect("first");
        let r2 = upsert(&pool, "dev-1", "https://e/a", "k2", "a2")
            .await
            .expect("second");
        assert_eq!(r1.id, r2.id, "同 endpoint upsert 应保 id 不变");
        assert_eq!(r2.p256dh, "k2", "keys 应被更新");

        let active = list_active(&pool).await.expect("list");
        assert_eq!(active.len(), 1);
    }

    /// 重新 upsert 清除 silence_until（用户重订阅意味着设备状态重置）。
    #[tokio::test]
    async fn upsert_resets_silence_until() {
        let pool = fresh_pool().await;
        let _ = upsert(&pool, "dev-1", "https://e/a", "k1", "a1")
            .await
            .unwrap();
        set_silence_until(
            &pool,
            "dev-1",
            Some(Utc::now() + chrono::Duration::hours(1)),
        )
        .await
        .expect("silence");
        // 此时不在 active
        let active = list_active(&pool).await.expect("list");
        assert!(active.is_empty(), "静音中不该出现");

        let _ = upsert(&pool, "dev-1", "https://e/a", "k1", "a1")
            .await
            .unwrap();
        let active = list_active(&pool).await.expect("list");
        assert_eq!(active.len(), 1, "重 upsert 后应恢复 active");
    }

    /// silence_until 在未来 = 当前不 active；过期后 = 又 active。
    #[tokio::test]
    async fn silence_until_filters_active() {
        let pool = fresh_pool().await;
        let _ = upsert(&pool, "dev-1", "https://e/a", "k", "a")
            .await
            .unwrap();

        let past = Utc::now() - chrono::Duration::seconds(1);
        set_silence_until(&pool, "dev-1", Some(past)).await.unwrap();
        assert_eq!(
            list_active(&pool).await.unwrap().len(),
            1,
            "已过期应 active"
        );

        let future = Utc::now() + chrono::Duration::hours(1);
        set_silence_until(&pool, "dev-1", Some(future))
            .await
            .unwrap();
        assert!(
            list_active(&pool).await.unwrap().is_empty(),
            "未过期应不在 active 列表"
        );

        set_silence_until(&pool, "dev-1", None).await.unwrap();
        assert_eq!(
            list_active(&pool).await.unwrap().len(),
            1,
            "清除 silence 后应 active"
        );
    }
}
