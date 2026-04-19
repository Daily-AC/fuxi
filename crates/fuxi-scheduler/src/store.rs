//! 候簿 + 应期 —— `triggers` / `trigger_fires` 的 SQLite 持久层。
//!
//! `TriggerStore` 只做 CRUD 和熔断计数，不做 cron 解析或 fire 决策（那是 Keeper 的事）。
//!
//! 为什么单连接 pool：和 `fuxi-events::EventStore` 同理——`:memory:` 多连接=多库；
//! 文件模式下再开大 pool 的收益目前也很小，串行写更省 SQLITE_BUSY 抖动。

use crate::spec::TriggerSpec;
use crate::{Error, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use fuxi_core::trigger_lookup::TriggerLookup;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

const SCHEMA_SQL: &str = include_str!("../migrations/0001_triggers.sql");

/// 触发源——`trigger_fires.cause` 列值；与 `EventKind::TriggerFired::cause` 字符串一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FireCause {
    Scheduled,
    Manual,
    Webhook,
    Fs,
}

impl FireCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            FireCause::Scheduled => "scheduled",
            FireCause::Manual => "manual",
            FireCause::Webhook => "webhook",
            FireCause::Fs => "fs",
        }
    }

    /// 解析小写字符串为 FireCause；未命中返回 None。
    /// 不实现 `std::str::FromStr` 是为了保持返回 `Option`（无错误类型一致性）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "scheduled" => Some(Self::Scheduled),
            "manual" => Some(Self::Manual),
            "webhook" => Some(Self::Webhook),
            "fs" => Some(Self::Fs),
            _ => None,
        }
    }
}

/// 本次 fire 的结局——append-only 写入 `trigger_fires.status`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FireStatus {
    Dispatched,
    Skipped,
    Failed,
}

impl FireStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FireStatus::Dispatched => "dispatched",
            FireStatus::Skipped => "skipped",
            FireStatus::Failed => "failed",
        }
    }
}

/// 候簿的一行。`spec` 已解码成 `TriggerSpec`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerRow {
    pub id: String,
    pub spec: TriggerSpec,
    pub intent: String,
    pub session_id: Option<String>,
    pub enabled: bool,
    pub consecutive_failures: i64,
    pub max_failures: i64,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 应期的一行——fire 流水。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FireRecord {
    pub id: String,
    pub trigger_id: String,
    pub fired_at: DateTime<Utc>,
    pub cause: FireCause,
    pub status: FireStatus,
    pub error: Option<String>,
    pub payload: Option<serde_json::Value>,
}

/// 新 trigger 的参数包——`id` 由 caller 给（`crate::new_trigger_id()`）。
#[derive(Debug, Clone)]
pub struct NewTrigger {
    pub id: String,
    pub spec: TriggerSpec,
    pub intent: String,
    pub session_id: Option<String>,
    pub max_failures: Option<i64>,
}

/// 候簿 + 应期 的 SQLite 存储。clone 便宜（内部 `Arc<Pool>`）。
#[derive(Debug, Clone)]
pub struct TriggerStore {
    pool: SqlitePool,
}

impl TriggerStore {
    /// 测试专用：`:memory:` 单连接。
    pub async fn connect_memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// 文件库 + WAL + auto-migrate。
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

    async fn init_schema(&self) -> Result<()> {
        for stmt in split_sql(SCHEMA_SQL) {
            if stmt.trim().is_empty() {
                continue;
            }
            sqlx::query(&stmt).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// 登记新 trigger。
    pub async fn insert(&self, new: NewTrigger) -> Result<TriggerRow> {
        let now = Utc::now();
        let spec_json = serde_json::to_string(&new.spec)?;
        let kind = new.spec.kind_str();
        let max_failures = new.max_failures.unwrap_or(5);
        sqlx::query(
            "INSERT INTO triggers (id, kind, spec, intent, session_id, enabled, \
             consecutive_failures, max_failures, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, ?6, ?7)",
        )
        .bind(&new.id)
        .bind(kind)
        .bind(&spec_json)
        .bind(&new.intent)
        .bind(new.session_id.as_deref())
        .bind(max_failures)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(TriggerRow {
            id: new.id,
            spec: new.spec,
            intent: new.intent,
            session_id: new.session_id,
            enabled: true,
            consecutive_failures: 0,
            max_failures,
            last_fired_at: None,
            created_at: now,
        })
    }

    /// 列出所有 trigger（包括 disabled）。
    pub async fn list_all(&self) -> Result<Vec<TriggerRow>> {
        let rows = sqlx::query(
            "SELECT id, spec, intent, session_id, enabled, consecutive_failures, \
             max_failures, last_fired_at, created_at FROM triggers ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_trigger).collect()
    }

    /// 列出启用中的 trigger（Keeper 轮询用）。
    pub async fn list_enabled(&self) -> Result<Vec<TriggerRow>> {
        let rows = sqlx::query(
            "SELECT id, spec, intent, session_id, enabled, consecutive_failures, \
             max_failures, last_fired_at, created_at FROM triggers \
             WHERE enabled = 1 ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_trigger).collect()
    }

    /// 只列启用中的 cron 类 trigger（Keeper 的热路径）。
    pub async fn list_enabled_cron(&self) -> Result<Vec<TriggerRow>> {
        let rows = sqlx::query(
            "SELECT id, spec, intent, session_id, enabled, consecutive_failures, \
             max_failures, last_fired_at, created_at FROM triggers \
             WHERE enabled = 1 AND kind = 'cron'",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_trigger).collect()
    }

    /// 按 id 取。
    pub async fn get(&self, id: &str) -> Result<Option<TriggerRow>> {
        let row = sqlx::query(
            "SELECT id, spec, intent, session_id, enabled, consecutive_failures, \
             max_failures, last_fired_at, created_at FROM triggers WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_trigger).transpose()
    }

    /// 按 id 删除（连带 fires；fires 的 FK 无 CASCADE，这里手动）。
    pub async fn remove(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM trigger_fires WHERE trigger_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let res = sqlx::query("DELETE FROM triggers WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// 手动 enable/disable。
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let res = sqlx::query("UPDATE triggers SET enabled = ?2 WHERE id = ?1")
            .bind(id)
            .bind(if enabled { 1 } else { 0 })
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// 记录 fire 成功——清零熔断计数 + 更新 last_fired_at。
    pub async fn mark_success(&self, id: &str, at: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "UPDATE triggers SET consecutive_failures = 0, last_fired_at = ?2 WHERE id = ?1",
        )
        .bind(id)
        .bind(at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 记录 fire 失败——`consecutive_failures += 1`；若达 `max_failures` 则 enabled=0。
    /// 返回是否刚刚因此被熔断。
    pub async fn mark_failure(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE triggers SET consecutive_failures = consecutive_failures + 1 WHERE id = ?1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(
            "SELECT consecutive_failures, max_failures, enabled FROM triggers WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Err(Error::NotFound(id.to_string()));
        };
        let cf: i64 = row.try_get("consecutive_failures")?;
        let mf: i64 = row.try_get("max_failures")?;
        let en: i64 = row.try_get("enabled")?;
        let tripped = if en == 1 && cf >= mf {
            sqlx::query("UPDATE triggers SET enabled = 0 WHERE id = ?1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            true
        } else {
            false
        };
        tx.commit().await?;
        Ok(tripped)
    }

    /// Append 一条 fire 记录。
    pub async fn append_fire(&self, f: &FireRecord) -> Result<()> {
        let payload = match &f.payload {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        sqlx::query(
            "INSERT INTO trigger_fires (id, trigger_id, fired_at, cause, status, error, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&f.id)
        .bind(&f.trigger_id)
        .bind(f.fired_at.to_rfc3339())
        .bind(f.cause.as_str())
        .bind(f.status.as_str())
        .bind(&f.error)
        .bind(&payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 仅测试：暴露 pool 给 keeper 的集成测试调 raw SQL。
    #[cfg(test)]
    pub(crate) fn pool_for_test(&self) -> &SqlitePool {
        &self.pool
    }

    /// 按 trigger_id 列出 fire 历史（最新在前）。
    pub async fn list_fires(&self, trigger_id: &str) -> Result<Vec<FireRecord>> {
        let rows = sqlx::query(
            "SELECT id, trigger_id, fired_at, cause, status, error, payload \
             FROM trigger_fires WHERE trigger_id = ?1 ORDER BY fired_at DESC",
        )
        .bind(trigger_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_fire).collect()
    }
}

/// 实装 [`fuxi_core::TriggerLookup`] —— 让 fuxi-orchestrator 的 SystemEventBridge
/// 能通过 trait object 读 intent，不用反向依赖本 crate。
///
/// 内部错误（DB 不可达/解析失败）在此路径下视为"找不到"：桥只是想给玄女一个提示，
/// 宁可静默跳过也不要把底层错误传染到事件消费者。
#[async_trait]
impl TriggerLookup for TriggerStore {
    async fn intent(&self, id: &str) -> Option<String> {
        match self.get(id).await {
            Ok(Some(row)) => Some(row.intent),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, trigger = id, "TriggerStore::intent 读取失败");
                None
            }
        }
    }
}

fn row_to_trigger(row: sqlx::sqlite::SqliteRow) -> Result<TriggerRow> {
    let id: String = row.try_get("id")?;
    let spec_json: String = row.try_get("spec")?;
    let spec: TriggerSpec = serde_json::from_str(&spec_json)?;
    let intent: String = row.try_get("intent")?;
    let session_id: Option<String> = row.try_get("session_id")?;
    let enabled: i64 = row.try_get("enabled")?;
    let cf: i64 = row.try_get("consecutive_failures")?;
    let mf: i64 = row.try_get("max_failures")?;
    let last_fired_at: Option<String> = row.try_get("last_fired_at")?;
    let created_at: String = row.try_get("created_at")?;
    let last_fired_at = last_fired_at
        .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.with_timezone(&Utc)))
        .transpose()
        .map_err(|e| Error::Other(format!("bad last_fired_at: {e}")))?;
    let created_at = DateTime::parse_from_rfc3339(&created_at)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| Error::Other(format!("bad created_at: {e}")))?;
    Ok(TriggerRow {
        id,
        spec,
        intent,
        session_id,
        enabled: enabled != 0,
        consecutive_failures: cf,
        max_failures: mf,
        last_fired_at,
        created_at,
    })
}

fn row_to_fire(row: sqlx::sqlite::SqliteRow) -> Result<FireRecord> {
    let id: String = row.try_get("id")?;
    let trigger_id: String = row.try_get("trigger_id")?;
    let fired_at: String = row.try_get("fired_at")?;
    let cause: String = row.try_get("cause")?;
    let status: String = row.try_get("status")?;
    let error: Option<String> = row.try_get("error")?;
    let payload: Option<String> = row.try_get("payload")?;
    let fired_at = DateTime::parse_from_rfc3339(&fired_at)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| Error::Other(format!("bad fired_at: {e}")))?;
    let cause =
        FireCause::parse(&cause).ok_or_else(|| Error::Other(format!("bad cause: {cause}")))?;
    let status = match status.as_str() {
        "dispatched" => FireStatus::Dispatched,
        "skipped" => FireStatus::Skipped,
        "failed" => FireStatus::Failed,
        other => return Err(Error::Other(format!("bad status: {other}"))),
    };
    let payload = match payload {
        Some(s) => Some(serde_json::from_str::<serde_json::Value>(&s)?),
        None => None,
    };
    Ok(FireRecord {
        id,
        trigger_id,
        fired_at,
        cause,
        status,
        error,
        payload,
    })
}

fn split_sql(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for line in sql.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("--") || trimmed.is_empty() {
            continue;
        }
        buf.push_str(line);
        buf.push('\n');
        if line.trim_end().ends_with(';') {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_fire_id;
    use crate::new_trigger_id;
    use std::path::PathBuf;

    fn mk_cron_new() -> NewTrigger {
        NewTrigger {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "*/5 * * * *".into(),
                tz: None,
            },
            intent: "每 5 分钟做一次深呼吸".into(),
            session_id: None,
            max_failures: None,
        }
    }

    #[tokio::test]
    async fn insert_and_get_roundtrip() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let new = mk_cron_new();
        let id = new.id.clone();
        let row = store.insert(new).await.expect("insert");
        assert_eq!(row.id, id);
        assert!(row.enabled);
        assert_eq!(row.consecutive_failures, 0);
        assert_eq!(row.max_failures, 5);

        let got = store.get(&id).await.expect("get").expect("some");
        assert_eq!(got.id, id);
        match got.spec {
            TriggerSpec::Cron { expr, .. } => assert_eq!(expr, "*/5 * * * *"),
            other => panic!("expected cron, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_enabled_filters_disabled() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let a = store.insert(mk_cron_new()).await.expect("insert a").id;
        let b = store.insert(mk_cron_new()).await.expect("insert b").id;
        store.set_enabled(&a, false).await.expect("disable a");
        let enabled = store.list_enabled().await.expect("list enabled");
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, b);
        let all = store.list_all().await.expect("list all");
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn list_enabled_cron_excludes_other_kinds() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let cron_id = store.insert(mk_cron_new()).await.expect("cron").id;
        let _ = store
            .insert(NewTrigger {
                id: new_trigger_id(),
                spec: TriggerSpec::Webhook { secret: None },
                intent: "webhook".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("webhook");
        let _ = store
            .insert(NewTrigger {
                id: new_trigger_id(),
                spec: TriggerSpec::FsWatch {
                    path: PathBuf::from("/tmp/x"),
                    events: vec![],
                },
                intent: "fs".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("fs");
        let only_cron = store.list_enabled_cron().await.expect("list");
        assert_eq!(only_cron.len(), 1);
        assert_eq!(only_cron[0].id, cron_id);
    }

    #[tokio::test]
    async fn remove_kills_trigger_and_fires() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let id = store.insert(mk_cron_new()).await.expect("insert").id;
        let fire = FireRecord {
            id: new_fire_id(),
            trigger_id: id.clone(),
            fired_at: Utc::now(),
            cause: FireCause::Manual,
            status: FireStatus::Dispatched,
            error: None,
            payload: None,
        };
        store.append_fire(&fire).await.expect("append fire");
        assert_eq!(store.list_fires(&id).await.expect("fires").len(), 1);
        assert!(store.remove(&id).await.expect("remove"));
        assert!(store.get(&id).await.expect("get").is_none());
        assert!(store.list_fires(&id).await.expect("fires").is_empty());
    }

    #[tokio::test]
    async fn fire_append_and_list_descending() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let id = store.insert(mk_cron_new()).await.expect("insert").id;
        let t1 = Utc::now();
        let t2 = t1 + chrono::Duration::seconds(1);
        let f1 = FireRecord {
            id: new_fire_id(),
            trigger_id: id.clone(),
            fired_at: t1,
            cause: FireCause::Scheduled,
            status: FireStatus::Dispatched,
            error: None,
            payload: None,
        };
        let f2 = FireRecord {
            id: new_fire_id(),
            trigger_id: id.clone(),
            fired_at: t2,
            cause: FireCause::Scheduled,
            status: FireStatus::Dispatched,
            error: None,
            payload: None,
        };
        store.append_fire(&f1).await.expect("f1");
        store.append_fire(&f2).await.expect("f2");
        let fires = store.list_fires(&id).await.expect("list");
        assert_eq!(fires.len(), 2);
        // desc
        assert_eq!(fires[0].id, f2.id);
        assert_eq!(fires[1].id, f1.id);
    }

    #[tokio::test]
    async fn mark_success_resets_failures_and_updates_last_fired_at() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let id = store.insert(mk_cron_new()).await.expect("insert").id;
        // 搞几次失败
        for _ in 0..3 {
            store.mark_failure(&id).await.expect("fail");
        }
        let pre = store.get(&id).await.expect("get").expect("some");
        assert_eq!(pre.consecutive_failures, 3);
        let at = Utc::now();
        store.mark_success(&id, at).await.expect("succ");
        let post = store.get(&id).await.expect("get").expect("some");
        assert_eq!(post.consecutive_failures, 0);
        assert!(post.last_fired_at.is_some());
    }

    #[tokio::test]
    async fn consecutive_failures_trips_circuit_at_max() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let id = store.insert(mk_cron_new()).await.expect("insert").id;
        // default max_failures = 5
        for i in 0..4 {
            let tripped = store.mark_failure(&id).await.expect("fail");
            assert!(!tripped, "第 {} 次不该熔断", i + 1);
        }
        let row = store.get(&id).await.expect("get").expect("some");
        assert!(row.enabled, "还没到 5 次失败不该熔断");
        let tripped = store.mark_failure(&id).await.expect("5th fail");
        assert!(tripped, "第 5 次失败应该熔断");
        let row = store.get(&id).await.expect("get").expect("some");
        assert!(!row.enabled, "熔断后 enabled 应为 0");
        assert_eq!(row.consecutive_failures, 5);
    }

    #[tokio::test]
    async fn mark_failure_twice_after_trip_stays_disabled_returns_false() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let id = store.insert(mk_cron_new()).await.expect("insert").id;
        for _ in 0..5 {
            store.mark_failure(&id).await.expect("fail");
        }
        // 再失败一次：已经 disabled，不会再 "刚刚熔断"
        let tripped = store.mark_failure(&id).await.expect("6th");
        assert!(!tripped, "已 disabled 的 trigger 不再报新熔断");
    }

    #[tokio::test]
    async fn trigger_lookup_returns_intent_for_known_id() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let row = store.insert(mk_cron_new()).await.expect("insert");
        let got = <TriggerStore as TriggerLookup>::intent(&store, &row.id).await;
        assert_eq!(got.as_deref(), Some("每 5 分钟做一次深呼吸"));
    }

    #[tokio::test]
    async fn trigger_lookup_returns_none_for_unknown_id() {
        let store = TriggerStore::connect_memory().await.expect("connect");
        let got = <TriggerStore as TriggerLookup>::intent(&store, "trg_nope").await;
        assert!(got.is_none());
    }
}
