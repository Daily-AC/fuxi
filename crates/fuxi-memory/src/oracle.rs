//! 甲骨：玄女长期事实表 (`oracle_facts`) 的存取层。
//!
//! subject/predicate/object 三段式：`(user, prefers, 冰美式)` / `(xuannv, session_id, <uuid>)`。
//! 写入只 ADD；冲突走 [`OracleStore::supersede`]（老行 `valid_until=now` 再 insert 新行）。
//! 检索走两路：精确 subject 索引 + FTS5 `oracle_fts` 模糊搜。

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

/// 即将落库的新事实——调用方填 subject/predicate/object/source/confidence，
/// id/时间戳由 store 生成，保证单一来源。
#[derive(Debug, Clone)]
pub struct NewFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub source: String,
    /// 置信度 [0.0, 1.0]。越界会被 [`OracleStore::insert`] 拒绝。
    pub confidence: f32,
}

impl NewFact {
    /// 便捷构造，`source` 缺省 `manual`，`confidence = 0.8`。
    pub fn new(
        subject: impl Into<String>,
        predicate: impl Into<String>,
        object: impl Into<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            source: "manual".to_string(),
            confidence: 0.8,
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence;
        self
    }
}

/// 表里一条现行 / 历史事实的投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleFact {
    pub id: Uuid,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub source: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `None` = 仍生效；`Some(t)` = `t` 之后作废（被 supersede）。
    pub valid_until: Option<DateTime<Utc>>,
}

/// 甲骨存储。clone 便宜（内部 `Arc` 语义的 `SqlitePool`）。
#[derive(Debug, Clone)]
pub struct OracleStore {
    pool: SqlitePool,
}

impl OracleStore {
    /// `:memory:` 临时库，测试首选。
    pub async fn connect_memory() -> Result<Self> {
        // 为什么限制 max_connections = 1：`:memory:` 每条连接是独立库，
        // 多连接会看不到彼此的写入。和 fuxi-events 那边踩过一样的坑。
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// 连到文件库，自动 WAL + schema 初始化。
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

    /// 复用已有 `SqlitePool`——共享 schema（`fuxi up` 把甲骨和 events 放同一个 db 文件）。
    pub fn with_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn init_schema(&self) -> Result<()> {
        crate::init_schema(&self.pool).await
    }

    /// 插入一条事实。成功返回含 id / 时间戳的完整 `OracleFact`。
    pub async fn insert(&self, fact: NewFact) -> Result<OracleFact> {
        validate_confidence(fact.confidence)?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let iso = now.to_rfc3339();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO oracle_facts \
             (id, subject, predicate, object, source, confidence, created_at, updated_at, valid_until) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        )
        .bind(id.to_string())
        .bind(&fact.subject)
        .bind(&fact.predicate)
        .bind(&fact.object)
        .bind(&fact.source)
        .bind(fact.confidence as f64)
        .bind(&iso)
        .bind(&iso)
        .execute(&mut *tx)
        .await?;

        insert_fts(&mut tx, id, &fact.subject, &fact.predicate, &fact.object).await?;
        tx.commit().await?;

        Ok(OracleFact {
            id,
            subject: fact.subject,
            predicate: fact.predicate,
            object: fact.object,
            source: fact.source,
            confidence: fact.confidence,
            created_at: now,
            updated_at: now,
            valid_until: None,
        })
    }

    /// 精确查一个 id。
    pub async fn get(&self, id: Uuid) -> Result<Option<OracleFact>> {
        let row = sqlx::query(
            "SELECT id, subject, predicate, object, source, confidence, \
                    created_at, updated_at, valid_until \
             FROM oracle_facts WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_fact).transpose()
    }

    /// 按 subject 查**仍生效**（valid_until IS NULL）的事实，倒序按 `updated_at`。
    pub async fn query(&self, subject: &str, limit: i64) -> Result<Vec<OracleFact>> {
        let rows = sqlx::query(
            "SELECT id, subject, predicate, object, source, confidence, \
                    created_at, updated_at, valid_until \
             FROM oracle_facts \
             WHERE subject = ?1 AND valid_until IS NULL \
             ORDER BY updated_at DESC LIMIT ?2",
        )
        .bind(subject)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_fact).collect()
    }

    /// 按 subject + predicate 查最新一条**仍生效**的事实——用来读玄女 session_id
    /// 这种单值约定。无匹配返回 `None`。
    pub async fn query_one(&self, subject: &str, predicate: &str) -> Result<Option<OracleFact>> {
        let row = sqlx::query(
            "SELECT id, subject, predicate, object, source, confidence, \
                    created_at, updated_at, valid_until \
             FROM oracle_facts \
             WHERE subject = ?1 AND predicate = ?2 AND valid_until IS NULL \
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(subject)
        .bind(predicate)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_fact).transpose()
    }

    /// FTS5 模糊搜。返回的只是仍生效（valid_until IS NULL）的条目。
    ///
    /// 查询串会先按同样的规则做 CJK 空格拆分，再包成 phrase 喂给 FTS5——
    /// 这样 `"玄女"` 会变成 phrase `"玄 女"` 去匹配已经拆开存储的索引文本。
    pub async fn fts_search(&self, q: &str, limit: i64) -> Result<Vec<OracleFact>> {
        if q.trim().is_empty() {
            return Ok(Vec::new());
        }
        let phrase = fts_phrase(q);
        let rows = sqlx::query(
            "SELECT f.id, f.subject, f.predicate, f.object, f.source, f.confidence, \
                    f.created_at, f.updated_at, f.valid_until \
             FROM oracle_facts f \
             JOIN oracle_fts fts ON fts.fact_id = f.id \
             WHERE oracle_fts MATCH ?1 AND f.valid_until IS NULL \
             ORDER BY rank LIMIT ?2",
        )
        .bind(&phrase)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_fact).collect()
    }

    /// `supersede(old_id, new)`：事务内先把老行 `valid_until = now`，再插入新行。
    /// 返回新行；若 `old_id` 不存在或已失效，返回 `Error::NotFound`。
    pub async fn supersede(&self, old_id: Uuid, new: NewFact) -> Result<OracleFact> {
        validate_confidence(new.confidence)?;
        let mut tx = self.pool.begin().await?;

        // 只允许对还生效的条目做 supersede——否则两次 supersede 会把老失效时间覆写。
        let exists =
            sqlx::query("SELECT id FROM oracle_facts WHERE id = ?1 AND valid_until IS NULL")
                .bind(old_id.to_string())
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(Error::NotFound(old_id.to_string()));
        }

        let now = Utc::now();
        let iso = now.to_rfc3339();

        sqlx::query("UPDATE oracle_facts SET valid_until = ?1, updated_at = ?1 WHERE id = ?2")
            .bind(&iso)
            .bind(old_id.to_string())
            .execute(&mut *tx)
            .await?;
        // 老行的 FTS 索引不删——`fts_search` 在 JOIN 后会因 `valid_until IS NOT NULL`
        // 把它过滤掉，留着反而给未来「历史搜索」留了窗口。

        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO oracle_facts \
             (id, subject, predicate, object, source, confidence, created_at, updated_at, valid_until) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        )
        .bind(id.to_string())
        .bind(&new.subject)
        .bind(&new.predicate)
        .bind(&new.object)
        .bind(&new.source)
        .bind(new.confidence as f64)
        .bind(&iso)
        .bind(&iso)
        .execute(&mut *tx)
        .await?;
        insert_fts(&mut tx, id, &new.subject, &new.predicate, &new.object).await?;

        tx.commit().await?;

        Ok(OracleFact {
            id,
            subject: new.subject,
            predicate: new.predicate,
            object: new.object,
            source: new.source,
            confidence: new.confidence,
            created_at: now,
            updated_at: now,
            valid_until: None,
        })
    }

    /// 把 (subject, predicate) 下所有仍生效的事实标 `valid_until=now` 失效。
    ///
    /// 用于"忘掉某事"——比如 `fuxi xuannv refresh` 让玄女下次 fresh spawn
    /// （清掉 xuannv/session_id record 让 ensure_xuannv 走 fresh session 而非
    /// `cc --resume`）。返回受影响行数。
    pub async fn invalidate(&self, subject: &str, predicate: &str) -> Result<u64> {
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE oracle_facts SET valid_until = ?1, updated_at = ?1 \
             WHERE subject = ?2 AND predicate = ?3 AND valid_until IS NULL",
        )
        .bind(&now)
        .bind(subject)
        .bind(predicate)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// 调整置信度——delta 正负均可，结果夹在 [0.0, 1.0]。
    /// 同步更新 `updated_at`，方便 `query` 的排序继续反映最新命中。
    pub async fn update_confidence(&self, id: Uuid, delta: f32) -> Result<OracleFact> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT confidence FROM oracle_facts WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Err(Error::NotFound(id.to_string()));
        };
        let current: f64 = row.try_get("confidence")?;
        let new_conf = ((current as f32) + delta).clamp(0.0, 1.0);
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE oracle_facts SET confidence = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(new_conf as f64)
            .bind(&now)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        self.get(id)
            .await?
            .ok_or_else(|| Error::NotFound(id.to_string()))
    }
}

fn validate_confidence(c: f32) -> Result<()> {
    if !(0.0..=1.0).contains(&c) || c.is_nan() {
        return Err(Error::InvalidArgument(format!(
            "confidence out of [0,1]: {c}"
        )));
    }
    Ok(())
}

fn row_to_fact(row: sqlx::sqlite::SqliteRow) -> Result<OracleFact> {
    let id_s: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_s).map_err(|e| Error::Other(format!("bad uuid: {e}")))?;
    let subject: String = row.try_get("subject")?;
    let predicate: String = row.try_get("predicate")?;
    let object: String = row.try_get("object")?;
    let source: String = row.try_get("source")?;
    let confidence: f64 = row.try_get("confidence")?;
    let created_at_s: String = row.try_get("created_at")?;
    let updated_at_s: String = row.try_get("updated_at")?;
    let valid_until_s: Option<String> = row.try_get("valid_until")?;
    let created_at = parse_ts(&created_at_s)?;
    let updated_at = parse_ts(&updated_at_s)?;
    let valid_until = match valid_until_s {
        Some(s) => Some(parse_ts(&s)?),
        None => None,
    };
    Ok(OracleFact {
        id,
        subject,
        predicate,
        object,
        source,
        confidence: confidence as f32,
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

/// 把用户输入转成 FTS5 的 phrase 表达式：先 CJK 空格拆分，再去双引号，再加两端引号。
/// 这样查询 `玄女` → `"玄 女"`，匹配我们在 insert 时也按同规则拆分过的索引文本。
fn fts_phrase(q: &str) -> String {
    let tokenized = spacify_cjk(q);
    let cleaned: String = tokenized.chars().filter(|c| *c != '"').collect();
    let trimmed = cleaned.trim();
    format!("\"{trimmed}\"")
}

/// 把 subject+predicate+object 拼成用于 FTS 索引的单列文本。
fn compose_search_text(subject: &str, predicate: &str, object: &str) -> String {
    let src = format!("{subject} {predicate} {object}");
    spacify_cjk(&src)
}

/// 在每个 CJK 字符两侧补空格。这样 unicode61 默认分词会把每个 CJK 字符单独作 token。
///
/// 判定范围包含：CJK 统一汉字区（U+4E00-U+9FFF）、扩展 A（U+3400-U+4DBF）、
/// 扩展 B-G（U+20000 起的四位 plane），以及常用中日韩符号（如 "，。"）。
/// 对 ASCII/拉丁保持原样——unicode61 对 `prefers` 这类还能按整词 token 匹配。
fn spacify_cjk(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        if is_cjk(ch) {
            out.push(' ');
            out.push(ch);
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x3400..=0x4DBF   // CJK Extension A
        | 0x4E00..=0x9FFF // CJK Unified
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0x20000..=0x2FFFF // CJK Extensions B-F
    )
}

/// 往 oracle_fts 写一条。调用方要持有事务——保证两张表一致。
async fn insert_fts(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fact_id: Uuid,
    subject: &str,
    predicate: &str,
    object: &str,
) -> Result<()> {
    let text = compose_search_text(subject, predicate, object);
    sqlx::query("INSERT INTO oracle_fts (fact_id, search_text) VALUES (?1, ?2)")
        .bind(fact_id.to_string())
        .bind(text)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
