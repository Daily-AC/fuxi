//! 河图洛书：门客经验表 (`hetu_patterns`) 的存取层。
//!
//! 每条是 `(role, task_type, pattern, outcome, confidence)`——门客做完一类任务
//! 后抽出的"我这样做成了 / 没做成"的小经验。高置信 + 高 hit 的条目可以由调用
//! 方写入 `skills/<role>/examples/`，并通过 [`HetuStore::promote`] 把状态标记
//! 成 "已晋升"——表本身不做文件操作，只记 bool。

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

/// 新记录——id / 时间戳由 store 统一生成。
#[derive(Debug, Clone)]
pub struct NewPattern {
    pub role: String,
    pub task_type: String,
    pub pattern: String,
    pub outcome: String,
    pub confidence: f32,
}

impl NewPattern {
    pub fn new(
        role: impl Into<String>,
        task_type: impl Into<String>,
        pattern: impl Into<String>,
        outcome: impl Into<String>,
    ) -> Self {
        Self {
            role: role.into(),
            task_type: task_type.into(),
            pattern: pattern.into(),
            outcome: outcome.into(),
            confidence: 0.6,
        }
    }

    pub fn with_confidence(mut self, c: f32) -> Self {
        self.confidence = c;
        self
    }
}

/// 落库后的完整投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HetuPattern {
    pub id: Uuid,
    pub role: String,
    pub task_type: String,
    pub pattern: String,
    pub outcome: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub promoted_to_skill: bool,
}

/// 河图洛书存储。clone 便宜。
#[derive(Debug, Clone)]
pub struct HetuStore {
    pool: SqlitePool,
}

impl HetuStore {
    pub async fn connect_memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

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

    pub fn with_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn init_schema(&self) -> Result<()> {
        crate::init_schema(&self.pool).await
    }

    /// 写一条经验。
    pub async fn record(&self, p: NewPattern) -> Result<HetuPattern> {
        if !(0.0..=1.0).contains(&p.confidence) || p.confidence.is_nan() {
            return Err(Error::InvalidArgument(format!(
                "confidence out of [0,1]: {}",
                p.confidence
            )));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let iso = now.to_rfc3339();
        sqlx::query(
            "INSERT INTO hetu_patterns \
             (id, role, task_type, pattern, outcome, confidence, created_at, promoted_to_skill) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        )
        .bind(id.to_string())
        .bind(&p.role)
        .bind(&p.task_type)
        .bind(&p.pattern)
        .bind(&p.outcome)
        .bind(p.confidence as f64)
        .bind(&iso)
        .execute(&self.pool)
        .await?;

        Ok(HetuPattern {
            id,
            role: p.role,
            task_type: p.task_type,
            pattern: p.pattern,
            outcome: p.outcome,
            confidence: p.confidence,
            created_at: now,
            promoted_to_skill: false,
        })
    }

    /// 按 role + task_type 查，置信度降序、时间降序。`task_type` 空串 = 不过滤。
    pub async fn query(&self, role: &str, task_type: &str) -> Result<Vec<HetuPattern>> {
        // 两条分支为什么不合一：`task_type = ''` 在 sqlx bind 上需要再写一个 OR；
        // 拆开两条 prepared statement 语义更直白也避免计划器重编。
        let rows = if task_type.is_empty() {
            sqlx::query(
                "SELECT id, role, task_type, pattern, outcome, confidence, created_at, promoted_to_skill \
                 FROM hetu_patterns WHERE role = ?1 \
                 ORDER BY confidence DESC, created_at DESC",
            )
            .bind(role)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, role, task_type, pattern, outcome, confidence, created_at, promoted_to_skill \
                 FROM hetu_patterns WHERE role = ?1 AND task_type = ?2 \
                 ORDER BY confidence DESC, created_at DESC",
            )
            .bind(role)
            .bind(task_type)
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter().map(row_to_pattern).collect()
    }

    /// 全表列出，按 created_at 降序——给 `fuxi memory list` 用。
    pub async fn list_all(&self, limit: i64) -> Result<Vec<HetuPattern>> {
        let rows = sqlx::query(
            "SELECT id, role, task_type, pattern, outcome, confidence, created_at, promoted_to_skill \
             FROM hetu_patterns ORDER BY created_at DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_pattern).collect()
    }

    /// 把指定条目标记为"已晋升 skill example"。幂等——重复 promote 不报错。
    pub async fn promote(&self, id: Uuid) -> Result<HetuPattern> {
        let rows = sqlx::query("UPDATE hetu_patterns SET promoted_to_skill = 1 WHERE id = ?1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if rows == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        let row = sqlx::query(
            "SELECT id, role, task_type, pattern, outcome, confidence, created_at, promoted_to_skill \
             FROM hetu_patterns WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await?;
        row_to_pattern(row)
    }
}

fn row_to_pattern(row: sqlx::sqlite::SqliteRow) -> Result<HetuPattern> {
    let id_s: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_s).map_err(|e| Error::Other(format!("bad uuid: {e}")))?;
    let role: String = row.try_get("role")?;
    let task_type: String = row.try_get("task_type")?;
    let pattern: String = row.try_get("pattern")?;
    let outcome: String = row.try_get("outcome")?;
    let confidence: f64 = row.try_get("confidence")?;
    let created_at_s: String = row.try_get("created_at")?;
    let promoted: i64 = row.try_get("promoted_to_skill")?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::Other(format!("bad ts: {e}")))?;
    Ok(HetuPattern {
        id,
        role,
        task_type,
        pattern,
        outcome,
        confidence: confidence as f32,
        created_at,
        promoted_to_skill: promoted != 0,
    })
}
