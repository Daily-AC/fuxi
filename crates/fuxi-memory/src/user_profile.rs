//! 用户身份卡 (`user_profile`)：论文 Memory Transfer Learning 的 Summary 层。
//!
//! 跟 `oracle_facts` 严格分流：
//! - oracle 是零碎事实三元组（`(user, prefers, 冰美式)`）；
//! - user_profile 是凝练身份卡（`identity → "以琳，工程师，主管产品；爱直球反馈"`）。
//!
//! 写入只 ADD：同 key 冲突走 [`UserProfileStore::supersede`]——老行
//! `valid_until = now` 再 insert 新行。`get` / `list_active` 只看活行。
//!
//! [`UserProfileStore::summary`] 输出 ≤200 字身份卡，spawn 门客时注入 prompt 用。

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

/// 即将落库的新身份卡条目。id / 时间戳由 store 生成。
#[derive(Debug, Clone)]
pub struct NewProfile {
    pub key: String,
    pub value: String,
    pub source: String,
}

impl NewProfile {
    /// 便捷构造，`source` 默认 `manual`。
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            source: "manual".to_string(),
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }
}

/// 表里一条现行 / 历史身份卡条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfileEntry {
    pub id: Uuid,
    pub key: String,
    pub value: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `None` = 仍生效；`Some(t)` = 在 `t` 被 supersede 失效。
    pub valid_until: Option<DateTime<Utc>>,
}

/// 身份卡存储。clone 便宜（内部 `Arc` 语义的 `SqlitePool`）。
#[derive(Debug, Clone)]
pub struct UserProfileStore {
    pool: SqlitePool,
}

impl UserProfileStore {
    /// `:memory:` 临时库，单测首选。
    pub async fn connect_memory() -> Result<Self> {
        // 同 oracle.rs 注释：`:memory:` 每条连接独立库，必须 max_connections=1
        // 否则多连接看不到彼此写入。
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// 文件库 + WAL + schema 自动初始化。
    pub async fn connect_file(path: impl AsRef<Path>) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path.as_ref())
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// 复用已有 pool（`fuxi up` 把策府 + events 放同一个 db 文件时走这条）。
    pub fn with_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn init_schema(&self) -> Result<()> {
        crate::init_schema(&self.pool).await
    }

    /// 写一条新身份卡。返回新行 id。**不**自动 supersede 同 key 老行——
    /// 那是 [`Self::supersede`] 的职责，调用方决定语义（追加还是替换）。
    pub async fn record(&self, new: NewProfile) -> Result<Uuid> {
        if new.key.trim().is_empty() {
            return Err(Error::InvalidArgument("key 不能为空".to_string()));
        }
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO user_profile \
             (id, key, value, source, created_at, updated_at, valid_until) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL)",
        )
        .bind(id.to_string())
        .bind(&new.key)
        .bind(&new.value)
        .bind(&new.source)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// 取某 key 当前**仍生效**的最新一条。同 key 多活行时取 `updated_at` 最大者。
    pub async fn get(&self, key: &str) -> Result<Option<UserProfileEntry>> {
        let row = sqlx::query(
            "SELECT id, key, value, source, created_at, updated_at, valid_until \
             FROM user_profile \
             WHERE key = ?1 AND valid_until IS NULL \
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_entry).transpose()
    }

    /// 列出所有**仍生效**的身份卡条目，按 `updated_at` 降序。
    pub async fn list_active(&self) -> Result<Vec<UserProfileEntry>> {
        let rows = sqlx::query(
            "SELECT id, key, value, source, created_at, updated_at, valid_until \
             FROM user_profile \
             WHERE valid_until IS NULL \
             ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_entry).collect()
    }

    /// 把 `id` 这条标失效但**不**插新行——`fuxi profile unset` 用。
    /// supersede 是 replace 语义（老行失效 + 新行接位），expire 是纯 delete 语义
    /// （以"valid_until = now"软删）。老行不存在或已失效 → `Error::NotFound`。
    pub async fn expire(&self, id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE user_profile SET valid_until = ?1, updated_at = ?1 \
             WHERE id = ?2 AND valid_until IS NULL",
        )
        .bind(&now)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// 把 `id` 这条标失效，并以同 key + 同 source 写一条新值。返回新行 id。
    /// 老行不存在或已失效 → `Error::NotFound`。
    pub async fn supersede(&self, id: Uuid, new_value: &str) -> Result<Uuid> {
        let mut tx = self.pool.begin().await?;

        // 只允许对仍生效的条目做 supersede——避免两次 supersede 把老 valid_until 覆写。
        let row = sqlx::query(
            "SELECT key, source FROM user_profile \
             WHERE id = ?1 AND valid_until IS NULL",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(Error::NotFound(id.to_string()));
        };
        let key: String = row.try_get("key")?;
        let source: String = row.try_get("source")?;

        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE user_profile SET valid_until = ?1, updated_at = ?1 WHERE id = ?2")
            .bind(&now)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;

        let new_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO user_profile \
             (id, key, value, source, created_at, updated_at, valid_until) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, NULL)",
        )
        .bind(new_id.to_string())
        .bind(&key)
        .bind(new_value)
        .bind(&source)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(new_id)
    }

    /// ≤200 字身份卡——spawn 门客时注入 prompt 的入口。
    ///
    /// 规则：列出所有活行，按 `key: value` 拼接，逗号分隔；超过 200 字按字符
    /// 截断（带省略号）。0 条返回空串，调用方决定是否注入。
    pub async fn summary(&self) -> Result<String> {
        let entries = self.list_active().await?;
        if entries.is_empty() {
            return Ok(String::new());
        }
        let joined: String = entries
            .iter()
            .map(|e| format!("{}: {}", e.key, e.value))
            .collect::<Vec<_>>()
            .join("；");
        Ok(truncate_chars(&joined, 200))
    }
}

/// 按 unicode scalar 切——而非字节——避免在 CJK 中间截断生成乱码。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    // 留一个字符位给省略号
    let cut = max_chars.saturating_sub(1);
    let mut out: String = chars[..cut].iter().collect();
    out.push('…');
    out
}

fn row_to_entry(row: sqlx::sqlite::SqliteRow) -> Result<UserProfileEntry> {
    let id_s: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_s).map_err(|e| Error::Other(format!("bad uuid: {e}")))?;
    let key: String = row.try_get("key")?;
    let value: String = row.try_get("value")?;
    let source: String = row.try_get("source")?;
    let created_at_s: String = row.try_get("created_at")?;
    let updated_at_s: String = row.try_get("updated_at")?;
    let valid_until_s: Option<String> = row.try_get("valid_until")?;
    let created_at = parse_ts(&created_at_s)?;
    let updated_at = parse_ts(&updated_at_s)?;
    let valid_until = match valid_until_s {
        Some(s) => Some(parse_ts(&s)?),
        None => None,
    };
    Ok(UserProfileEntry {
        id,
        key,
        value,
        source,
        created_at,
        updated_at,
        valid_until,
    })
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::Other(format!("bad timestamp {s:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> UserProfileStore {
        UserProfileStore::connect_memory()
            .await
            .expect("connect memory")
    }

    #[tokio::test]
    async fn record_then_get() {
        let s = store().await;
        let id = s
            .record(NewProfile::new("identity", "以琳，工程师"))
            .await
            .unwrap();
        let got = s.get("identity").await.unwrap().unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.value, "以琳，工程师");
        assert_eq!(got.source, "manual");
        assert!(got.valid_until.is_none());
    }

    #[tokio::test]
    async fn record_rejects_empty_key() {
        let s = store().await;
        let err = s.record(NewProfile::new("", "v")).await.err().unwrap();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn get_returns_none_when_missing() {
        let s = store().await;
        assert!(s.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_active_orders_by_updated_desc() {
        let s = store().await;
        s.record(NewProfile::new("a", "1")).await.unwrap();
        // 不同条目 updated_at 拉开间距，避免毫秒级并列导致顺序抖动。
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let id_b = s.record(NewProfile::new("b", "2")).await.unwrap();

        let rows = s.list_active().await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, id_b, "最新写入排前");
    }

    #[tokio::test]
    async fn supersede_replaces_active_entry() {
        let s = store().await;
        let old = s
            .record(NewProfile::new("identity", "v1").with_source("cangjie-auto"))
            .await
            .unwrap();
        let new = s.supersede(old, "v2").await.unwrap();

        // get 只能看到新行
        let got = s.get("identity").await.unwrap().unwrap();
        assert_eq!(got.id, new);
        assert_eq!(got.value, "v2");
        // 新行继承老行的 source
        assert_eq!(got.source, "cangjie-auto");

        // list_active 只剩 1 条
        let rows = s.list_active().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, new);
    }

    #[tokio::test]
    async fn supersede_unknown_returns_not_found() {
        let s = store().await;
        let err = s.supersede(Uuid::new_v4(), "x").await.err().unwrap();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn double_supersede_uses_chain() {
        let s = store().await;
        let v1 = s.record(NewProfile::new("k", "v1")).await.unwrap();
        let v2 = s.supersede(v1, "v2").await.unwrap();
        let v3 = s.supersede(v2, "v3").await.unwrap();

        let got = s.get("k").await.unwrap().unwrap();
        assert_eq!(got.id, v3);
        assert_eq!(got.value, "v3");

        // 二次 supersede 老 id 必须报 NotFound——它已被第一次 supersede 标失效。
        let err = s.supersede(v1, "x").await.err().unwrap();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn summary_empty_when_no_entries() {
        let s = store().await;
        assert_eq!(s.summary().await.unwrap(), "");
    }

    #[tokio::test]
    async fn summary_joins_active_entries() {
        let s = store().await;
        s.record(NewProfile::new("identity", "以琳")).await.unwrap();
        s.record(NewProfile::new("tone", "直球")).await.unwrap();
        let sum = s.summary().await.unwrap();
        assert!(sum.contains("identity: 以琳"));
        assert!(sum.contains("tone: 直球"));
    }

    #[tokio::test]
    async fn summary_truncates_to_200_chars() {
        let s = store().await;
        // 一条 250 个汉字的 value——保证总长度肯定超过 200 字。
        let long_val: String = "字".repeat(250);
        s.record(NewProfile::new("identity", &long_val))
            .await
            .unwrap();
        let sum = s.summary().await.unwrap();
        let count = sum.chars().count();
        assert!(count <= 200, "summary {count} 字超 200");
        assert!(sum.ends_with('…'), "截断后必须带省略号");
    }

    #[tokio::test]
    async fn expire_marks_inactive_without_new_row() {
        let s = store().await;
        let id = s.record(NewProfile::new("identity", "v1")).await.unwrap();
        s.expire(id).await.unwrap();

        // get 看不见
        assert!(s.get("identity").await.unwrap().is_none());
        // list_active 也清空
        assert!(s.list_active().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn expire_unknown_returns_not_found() {
        let s = store().await;
        let err = s.expire(Uuid::new_v4()).await.err().unwrap();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn expire_already_expired_returns_not_found() {
        let s = store().await;
        let id = s.record(NewProfile::new("k", "v")).await.unwrap();
        s.expire(id).await.unwrap();
        let err = s.expire(id).await.err().unwrap();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn summary_excludes_superseded_entries() {
        let s = store().await;
        let old = s.record(NewProfile::new("identity", "v1")).await.unwrap();
        s.supersede(old, "v2").await.unwrap();
        let sum = s.summary().await.unwrap();
        assert!(sum.contains("v2"));
        assert!(!sum.contains("v1"));
    }
}
