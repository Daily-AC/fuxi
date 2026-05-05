//! 河图洛书：门客经验 / insight 表 (`hetu_patterns`) 的存取层。
//!
//! v1（招贤司路径）：每条是 `(role, task_type, pattern, outcome, confidence)`——
//! 门客做完一类活的小经验，高置信高 hit 可晋升为 skill example。
//!
//! v2（论文 arXiv:2604.14004 Memory Transfer Learning Insight 层）：扩展 4 字段：
//! - `abstraction_score` (Option<f64>)：LLM-as-judge 0.0-1.0；< 0.6 拒收
//! - `derived_from_task` (Option<String>)：关联 task uuid，可审计
//! - `source` (String)：'cangjie-auto' / 'manual' / ...
//! - `valid_until` (Option<DateTime<Utc>>)：supersede 用，跟甲骨同语义
//!
//! task_type 仍是 `String`（SQLite ALTER 改 NOT NULL → NULL 不支持）；应用层用
//! 空串约定 "task-agnostic insight"。`NewPattern::insight()` 默认就空串。

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
    /// 任务类别。空串 = task-agnostic insight（论文常见形态）。
    pub task_type: String,
    pub pattern: String,
    pub outcome: String,
    pub confidence: f32,
    /// LLM-as-judge 抽象度评分 0.0-1.0；仓颉自动提取走这条，手动入可空。
    pub abstraction_score: Option<f64>,
    /// 关联 task uuid（仓颉提取时填）。
    pub derived_from_task: Option<String>,
    /// 'cangjie-auto' / 'manual' / 'user' 等来源标签；默认 "manual"。
    pub source: String,
}

impl NewPattern {
    /// 招贤司路径（v1 兼容）：经验三元组 + 验证结果。
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
            abstraction_score: None,
            derived_from_task: None,
            source: "manual".to_string(),
        }
    }

    /// 仓颉路径（论文 Insight 层）：默认 task-agnostic + outcome="success" +
    /// source="cangjie-auto"。配合 `with_*` builder 链式补字段。
    pub fn insight(role: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            task_type: String::new(),
            pattern: pattern.into(),
            outcome: "success".to_string(),
            confidence: 0.5,
            abstraction_score: None,
            derived_from_task: None,
            source: "cangjie-auto".to_string(),
        }
    }

    pub fn with_confidence(mut self, c: f32) -> Self {
        self.confidence = c;
        self
    }

    pub fn with_task_type(mut self, t: impl Into<String>) -> Self {
        self.task_type = t.into();
        self
    }

    pub fn with_abstraction_score(mut self, s: f64) -> Self {
        self.abstraction_score = Some(s);
        self
    }

    pub fn with_derived_from_task(mut self, t: impl Into<String>) -> Self {
        self.derived_from_task = Some(t.into());
        self
    }

    pub fn with_source(mut self, s: impl Into<String>) -> Self {
        self.source = s.into();
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
    pub abstraction_score: Option<f64>,
    pub derived_from_task: Option<String>,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
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

    /// 写一条经验 / insight。校验：confidence ∈ [0,1]；abstraction_score（若有）∈ [0,1]。
    pub async fn record(&self, p: NewPattern) -> Result<HetuPattern> {
        if !(0.0..=1.0).contains(&p.confidence) || p.confidence.is_nan() {
            return Err(Error::InvalidArgument(format!(
                "confidence out of [0,1]: {}",
                p.confidence
            )));
        }
        if let Some(s) = p.abstraction_score
            && (!(0.0..=1.0).contains(&s) || s.is_nan())
        {
            return Err(Error::InvalidArgument(format!(
                "abstraction_score out of [0,1]: {s}"
            )));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let iso = now.to_rfc3339();
        sqlx::query(
            "INSERT INTO hetu_patterns \
             (id, role, task_type, pattern, outcome, confidence, \
              abstraction_score, derived_from_task, source, \
              created_at, valid_until, promoted_to_skill) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, 0)",
        )
        .bind(id.to_string())
        .bind(&p.role)
        .bind(&p.task_type)
        .bind(&p.pattern)
        .bind(&p.outcome)
        .bind(p.confidence as f64)
        .bind(p.abstraction_score)
        .bind(&p.derived_from_task)
        .bind(&p.source)
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
            abstraction_score: p.abstraction_score,
            derived_from_task: p.derived_from_task,
            source: p.source,
            created_at: now,
            valid_until: None,
            promoted_to_skill: false,
        })
    }

    /// 按 role + task_type 查（仅 active = `valid_until IS NULL`），置信度 + 抽象度 + 时间降序。
    /// `task_type` 空串 = 不过滤。
    pub async fn query(&self, role: &str, task_type: &str) -> Result<Vec<HetuPattern>> {
        let rows = if task_type.is_empty() {
            sqlx::query(SELECT_ACTIVE_BY_ROLE)
                .bind(role)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query(SELECT_ACTIVE_BY_ROLE_AND_TASK)
                .bind(role)
                .bind(task_type)
                .fetch_all(&self.pool)
                .await?
        };
        rows.into_iter().map(row_to_pattern).collect()
    }

    /// 仓颉 / spawn 注入桥用：拿某 role 最近 N 条 active insight，按 abstraction_score
    /// + created_at 降序——抽象度高的先注入门客 prompt（论文核心：抽象度决定迁移性）。
    pub async fn recent_for_role(&self, role: &str, limit: usize) -> Result<Vec<HetuPattern>> {
        let rows = sqlx::query(
            "SELECT id, role, task_type, pattern, outcome, confidence, \
                    abstraction_score, derived_from_task, source, \
                    created_at, valid_until, promoted_to_skill \
             FROM hetu_patterns \
             WHERE role = ?1 AND valid_until IS NULL \
             ORDER BY abstraction_score IS NULL, abstraction_score DESC, \
                      created_at DESC \
             LIMIT ?2",
        )
        .bind(role)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_pattern).collect()
    }

    /// 全表列出（含 superseded，给 `fuxi memory list` 用）；`active_only=true`
    /// 则仅 valid_until IS NULL。按 created_at 降序。
    pub async fn list_all(&self, limit: i64) -> Result<Vec<HetuPattern>> {
        let rows = sqlx::query(
            "SELECT id, role, task_type, pattern, outcome, confidence, \
                    abstraction_score, derived_from_task, source, \
                    created_at, valid_until, promoted_to_skill \
             FROM hetu_patterns ORDER BY created_at DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_pattern).collect()
    }

    /// 仅 active（valid_until IS NULL），创建时间降序——`fuxi insight list` 默认用这条。
    pub async fn list_active(&self, limit: i64) -> Result<Vec<HetuPattern>> {
        let rows = sqlx::query(
            "SELECT id, role, task_type, pattern, outcome, confidence, \
                    abstraction_score, derived_from_task, source, \
                    created_at, valid_until, promoted_to_skill \
             FROM hetu_patterns WHERE valid_until IS NULL \
             ORDER BY created_at DESC LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_pattern).collect()
    }

    /// 标过期：把 `valid_until` 填到此刻，记录从 active 视图中消失但行保留供审计。
    /// 幂等——重复 supersede 同一 id 等于刷新过期时刻。
    pub async fn supersede(&self, id: Uuid) -> Result<()> {
        let now_iso = Utc::now().to_rfc3339();
        let rows = sqlx::query("UPDATE hetu_patterns SET valid_until = ?1 WHERE id = ?2")
            .bind(&now_iso)
            .bind(id.to_string())
            .execute(&self.pool)
            .await?
            .rows_affected();
        if rows == 0 {
            return Err(Error::NotFound(id.to_string()));
        }
        Ok(())
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
            "SELECT id, role, task_type, pattern, outcome, confidence, \
                    abstraction_score, derived_from_task, source, \
                    created_at, valid_until, promoted_to_skill \
             FROM hetu_patterns WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await?;
        row_to_pattern(row)
    }
}

const SELECT_ACTIVE_BY_ROLE: &str = "SELECT id, role, task_type, pattern, outcome, confidence, \
            abstraction_score, derived_from_task, source, \
            created_at, valid_until, promoted_to_skill \
     FROM hetu_patterns WHERE role = ?1 AND valid_until IS NULL \
     ORDER BY confidence DESC, abstraction_score IS NULL, abstraction_score DESC, created_at DESC";

const SELECT_ACTIVE_BY_ROLE_AND_TASK: &str = "SELECT id, role, task_type, pattern, outcome, confidence, \
            abstraction_score, derived_from_task, source, \
            created_at, valid_until, promoted_to_skill \
     FROM hetu_patterns WHERE role = ?1 AND task_type = ?2 AND valid_until IS NULL \
     ORDER BY confidence DESC, abstraction_score IS NULL, abstraction_score DESC, created_at DESC";

fn row_to_pattern(row: sqlx::sqlite::SqliteRow) -> Result<HetuPattern> {
    let id_s: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_s).map_err(|e| Error::Other(format!("bad uuid: {e}")))?;
    let role: String = row.try_get("role")?;
    let task_type: String = row.try_get("task_type")?;
    let pattern: String = row.try_get("pattern")?;
    let outcome: String = row.try_get("outcome")?;
    let confidence: f64 = row.try_get("confidence")?;
    // 显式 turbofish——`.ok()` + 类型推断会把 NULL 列上的 try_get<String> 错误
    // 误吞，结果落成 Some("")，下游 chrono parse 撞 "premature end of input"。
    // 直接走 try_get::<Option<T>, _>：NULL → Ok(None)，非 NULL → Ok(Some(v))。
    let abstraction_score: Option<f64> = row.try_get::<Option<f64>, _>("abstraction_score")?;
    let derived_from_task: Option<String> =
        row.try_get::<Option<String>, _>("derived_from_task")?;
    let source: Option<String> = row.try_get::<Option<String>, _>("source")?;
    let created_at_s: String = row.try_get("created_at")?;
    let valid_until_s: Option<String> = row.try_get::<Option<String>, _>("valid_until")?;
    let promoted: i64 = row.try_get("promoted_to_skill")?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::Other(format!("bad ts: {e}")))?;
    let valid_until = valid_until_s
        .as_deref()
        .map(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| Error::Other(format!("bad valid_until: {e}")))
        })
        .transpose()?;
    Ok(HetuPattern {
        id,
        role,
        task_type,
        pattern,
        outcome,
        confidence: confidence as f32,
        abstraction_score,
        derived_from_task,
        source: source.unwrap_or_else(|| "manual".to_string()),
        created_at,
        valid_until,
        promoted_to_skill: promoted != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> HetuStore {
        HetuStore::connect_memory().await.unwrap()
    }

    #[tokio::test]
    async fn record_and_query_legacy() {
        let s = store().await;
        let p = s
            .record(NewPattern::new(
                "luban",
                "refactor",
                "TDD 先红再绿",
                "success",
            ))
            .await
            .unwrap();
        assert_eq!(p.role, "luban");
        assert_eq!(p.source, "manual");
        assert!(p.abstraction_score.is_none());
        let q = s.query("luban", "refactor").await.unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].pattern, "TDD 先红再绿");
    }

    #[tokio::test]
    async fn insight_builder_defaults_match_cangjie_path() {
        let s = store().await;
        let new = NewPattern::insight("luban", "Cargo.lock 撞冲突 rm 重建快")
            .with_abstraction_score(0.85)
            .with_derived_from_task("task-abc");
        let p = s.record(new).await.unwrap();
        assert_eq!(p.role, "luban");
        assert_eq!(p.task_type, ""); // task-agnostic
        assert_eq!(p.outcome, "success");
        assert_eq!(p.source, "cangjie-auto");
        assert_eq!(p.abstraction_score, Some(0.85));
        assert_eq!(p.derived_from_task.as_deref(), Some("task-abc"));
    }

    #[tokio::test]
    async fn record_rejects_bad_abstraction_score() {
        let s = store().await;
        let new = NewPattern::insight("luban", "x").with_abstraction_score(1.5);
        let err = s.record(new).await.unwrap_err();
        assert!(format!("{err}").contains("abstraction_score"));
    }

    #[tokio::test]
    async fn recent_for_role_orders_by_abstraction_then_time() {
        let s = store().await;
        // 三条 luban：高抽象度晚加 / 低抽象度早加 / 无 score 中间
        s.record(NewPattern::insight("luban", "low-abs early").with_abstraction_score(0.4))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        s.record(NewPattern::insight("luban", "no-score mid"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        s.record(NewPattern::insight("luban", "high-abs late").with_abstraction_score(0.9))
            .await
            .unwrap();

        let recent = s.recent_for_role("luban", 5).await.unwrap();
        assert_eq!(recent.len(), 3);
        // 论文核心：高抽象度先出
        assert_eq!(recent[0].pattern, "high-abs late");
        assert_eq!(recent[1].pattern, "low-abs early");
        // None abstraction_score 排最后（NULLS LAST）
        assert_eq!(recent[2].pattern, "no-score mid");
    }

    #[tokio::test]
    async fn supersede_removes_from_active_views() {
        let s = store().await;
        let p = s.record(NewPattern::insight("luban", "x")).await.unwrap();
        s.supersede(p.id).await.unwrap();
        let recent = s.recent_for_role("luban", 5).await.unwrap();
        assert_eq!(recent.len(), 0, "superseded 不在 recent_for_role 视图中");
        let active = s.list_active(10).await.unwrap();
        assert_eq!(active.len(), 0);
        // 但 list_all 仍能看到（审计保留）
        let all = s.list_all(10).await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].valid_until.is_some());
    }

    #[tokio::test]
    async fn supersede_unknown_returns_not_found() {
        let s = store().await;
        let err = s.supersede(Uuid::new_v4()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn list_active_skips_superseded_ordered_by_created() {
        let s = store().await;
        let _p1 = s
            .record(NewPattern::insight("luban", "first"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let p2 = s
            .record(NewPattern::insight("luban", "second"))
            .await
            .unwrap();
        s.supersede(p2.id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _p3 = s
            .record(NewPattern::insight("luban", "third"))
            .await
            .unwrap();
        let active = s.list_active(10).await.unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].pattern, "third");
        assert_eq!(active[1].pattern, "first");
    }

    #[tokio::test]
    async fn promote_still_works_under_v2() {
        let s = store().await;
        let p = s
            .record(NewPattern::new("luban", "refactor", "TDD", "success"))
            .await
            .unwrap();
        let promoted = s.promote(p.id).await.unwrap();
        assert!(promoted.promoted_to_skill);
    }

    #[tokio::test]
    async fn migrate_idempotent() {
        // connect_memory 内部跑了 init_schema 一次；再手工跑一遍验证 ALTER 容错。
        let s = store().await;
        // 通过 with_pool 复用同一 pool 再 init—— migrate 该 ignore duplicate column。
        let s2 = HetuStore::with_pool(s.pool.clone());
        s2.init_schema().await.unwrap();
        // 仍能正常 record
        let p = s.record(NewPattern::insight("luban", "x")).await.unwrap();
        assert_eq!(p.role, "luban");
    }

    #[tokio::test]
    async fn query_excludes_superseded() {
        let s = store().await;
        let p = s
            .record(NewPattern::new("luban", "refactor", "x", "success"))
            .await
            .unwrap();
        let q1 = s.query("luban", "refactor").await.unwrap();
        assert_eq!(q1.len(), 1);
        s.supersede(p.id).await.unwrap();
        let q2 = s.query("luban", "refactor").await.unwrap();
        assert_eq!(q2.len(), 0, "supersede 后 query 不返");
    }
}
