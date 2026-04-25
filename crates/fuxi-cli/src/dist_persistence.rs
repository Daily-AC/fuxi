//! 分布式 controller 的 job 持久化层（path 4 α）。
//!
//! 公理 #5 要求 SQLite 是单一真相源；当前 `DistInner.global_queue` / `inflight`
//! 全是 in-memory，controller 重启 → 已 enqueue 但未 done 的 job 全丢，
//! 违反"gateway 重启 in-flight 不丢"承诺。
//!
//! 设计：dual-writer pattern——mutating ops（enqueue/pull/report/cancel/sweep）
//! **同时**写 in-memory cache 与 SQLite。in-memory 仍是 hot path（pull 不读盘），
//! SQLite 是 restart 时的真相源。restore 时把 'queued' 行重新塞回 queue，
//! 'inflight' 行视作 stale 重 enqueue（worker 早就死了，只能让别的 worker 重跑）。
//!
//! 为什么不进 fuxi-events：
//! 1. dist_jobs 是 fuxi-cli 层概念（fuxi-events 是更通用的 EventStore）
//! 2. dist 表 schema 演化节奏与 events 表完全无关，混在一起 migration 互相牵制
//!
//! BUSY retry：参 fuxi-events EventStore::append 的兜底——三次指数退避。

use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use tracing::{debug, error, warn};

use crate::dist::DistJob;

/// 内嵌 schema——与 fuxi-events EventStore 同套路（避免编译期路径依赖）。
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS dist_jobs (
    id TEXT PRIMARY KEY,
    payload TEXT NOT NULL,
    state TEXT NOT NULL,
    assignee TEXT,
    enqueued_at TEXT NOT NULL,
    dispatched_at TEXT,
    completed_at TEXT,
    ok INTEGER
);
CREATE INDEX IF NOT EXISTS idx_dist_jobs_state ON dist_jobs(state);
CREATE INDEX IF NOT EXISTS idx_dist_jobs_assignee ON dist_jobs(assignee);
"#;

/// dist_jobs.state 的字符串常量——避免散落 typo。
pub const STATE_QUEUED: &str = "queued";
pub const STATE_INFLIGHT: &str = "inflight";
pub const STATE_DONE: &str = "done";
pub const STATE_CANCELLED: &str = "cancelled";

/// restore 时返回的 job 集合——controller::restore_from_persistence 用来
/// 重建 in-memory 状态。
#[derive(Debug, Clone)]
pub struct RestoredJobs {
    /// 原本 state='queued' 的 job——按 enqueued_at 升序。
    pub queued: Vec<DistJob>,
    /// 原本 state='inflight' 的 job——controller 死之前 worker 在跑，
    /// 现在 controller 重启，worker 可能也死了/或心跳超时，统一当 stale
    /// 重 enqueue（被 sweep 当 orphan 走也是 OK 路径）。
    pub orphans: Vec<DistJob>,
}

/// SQLite 持久化句柄。clone 便宜（内部 Arc）。
#[derive(Debug, Clone)]
pub struct JobPersistence {
    pool: SqlitePool,
}

impl JobPersistence {
    /// 内存库——测试首选。
    pub async fn connect_memory() -> Result<Self, sqlx::Error> {
        // max_connections=1：SQLite `:memory:` 每条连接独立库，多连接读不到对方写。
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// 文件库——生产路径。WAL 模式 + busy_timeout 5s。
    pub async fn connect_file(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
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

    async fn init_schema(&self) -> Result<(), sqlx::Error> {
        for stmt in split_sql(SCHEMA_SQL) {
            if stmt.trim().is_empty() {
                continue;
            }
            sqlx::query(&stmt).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// enqueue：INSERT row state='queued'。重复 id 按 INSERT OR REPLACE 处理
    /// （理论 controller 自分配 uuid 不会撞，但兼容 restore 后再 enqueue 同 id 的 race）。
    pub async fn record_enqueue(&self, job: &DistJob) -> Result<(), sqlx::Error> {
        let payload = match serde_json::to_string(job) {
            Ok(s) => s,
            Err(e) => {
                error!(job_id = %job.id, error = %e, "dist_jobs payload 序列化失败");
                return Err(sqlx::Error::Protocol(format!("serialize job: {e}")));
            }
        };
        let now = Utc::now().to_rfc3339();
        with_busy_retry(|| async {
            sqlx::query(
                "INSERT OR REPLACE INTO dist_jobs (id, payload, state, assignee, enqueued_at, dispatched_at, completed_at, ok) \
                 VALUES (?1, ?2, ?3, NULL, ?4, NULL, NULL, NULL)",
            )
            .bind(&job.id)
            .bind(&payload)
            .bind(STATE_QUEUED)
            .bind(&now)
            .execute(&self.pool)
            .await
            .map(|_| ())
        })
        .await?;
        debug!(job_id = %job.id, "dist_jobs enqueue 写入");
        Ok(())
    }

    /// pull：UPDATE state='inflight', assignee=node, dispatched_at=now。
    /// id 不存在时返回 Ok（restart race：worker 拿到 in-memory job 但 SQLite 行还没写——
    /// 实际不会发生，因为 enqueue 先写 SQLite 再 push queue；保留兜底不报错）。
    pub async fn record_pull(&self, job_id: &str, node_id: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        with_busy_retry(|| async {
            sqlx::query(
                "UPDATE dist_jobs SET state = ?1, assignee = ?2, dispatched_at = ?3 WHERE id = ?4",
            )
            .bind(STATE_INFLIGHT)
            .bind(node_id)
            .bind(&now)
            .bind(job_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
        })
        .await?;
        Ok(())
    }

    /// report：UPDATE state='done', completed_at=now, ok=:result。assignee 保留
    /// （审计用——知道是哪台 worker 跑完的）。
    pub async fn record_report(&self, job_id: &str, ok: bool) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let ok_int: i64 = if ok { 1 } else { 0 };
        with_busy_retry(|| async {
            sqlx::query("UPDATE dist_jobs SET state = ?1, completed_at = ?2, ok = ?3 WHERE id = ?4")
                .bind(STATE_DONE)
                .bind(&now)
                .bind(ok_int)
                .bind(job_id)
                .execute(&self.pool)
                .await
                .map(|_| ())
        })
        .await?;
        Ok(())
    }

    /// cancel：UPDATE state='cancelled'。完成态保留 assignee/dispatched_at；
    /// queued 态没人接，assignee 仍为 NULL。
    /// 不**强制** worker 已经下发——cancelled 是"调度层意图"，runtime 由 heartbeat
    /// ack 路径执行。
    pub async fn record_cancel(&self, job_id: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        with_busy_retry(|| async {
            sqlx::query("UPDATE dist_jobs SET state = ?1, completed_at = ?2 WHERE id = ?3")
                .bind(STATE_CANCELLED)
                .bind(&now)
                .bind(job_id)
                .execute(&self.pool)
                .await
                .map(|_| ())
        })
        .await?;
        Ok(())
    }

    /// sweep：把 inflight 行回收回 queued（worker 死掉的兜底）。
    /// 现实场景中 sweep_stale 已经从 in-memory 把 job push_front 回 queue；
    /// 这里同步一行让 SQLite 不再认为它是 inflight。assignee 清空。
    pub async fn record_sweep_to_queued(&self, job_id: &str) -> Result<(), sqlx::Error> {
        with_busy_retry(|| async {
            sqlx::query(
                "UPDATE dist_jobs SET state = ?1, assignee = NULL, dispatched_at = NULL WHERE id = ?2",
            )
            .bind(STATE_QUEUED)
            .bind(job_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
        })
        .await?;
        Ok(())
    }

    /// restore：扫盘把未完成的 job 还原。
    ///
    /// - state='queued'：按 enqueued_at 升序返回（保留派工次序）
    /// - state='inflight'：当 stale orphan 返回——controller 死了之前在跑这个，
    ///   现在 worker 大概率也凉/或心跳过了；交由 controller 端重 enqueue，
    ///   后续 sweep_stale / 新 worker pull 会接管
    /// - state='done' / 'cancelled'：不动（历史记录）
    pub async fn restore(&self) -> Result<RestoredJobs, sqlx::Error> {
        let queued_rows = sqlx::query(
            "SELECT payload FROM dist_jobs WHERE state = ?1 ORDER BY enqueued_at ASC, id ASC",
        )
        .bind(STATE_QUEUED)
        .fetch_all(&self.pool)
        .await?;
        let mut queued = Vec::with_capacity(queued_rows.len());
        for row in queued_rows {
            let payload: String = row.try_get("payload")?;
            match serde_json::from_str::<DistJob>(&payload) {
                Ok(job) => queued.push(job),
                Err(e) => {
                    warn!(error = %e, "dist_jobs queued 行 payload 解析失败，跳过");
                }
            }
        }

        let inflight_rows = sqlx::query(
            "SELECT payload FROM dist_jobs WHERE state = ?1 ORDER BY dispatched_at ASC, id ASC",
        )
        .bind(STATE_INFLIGHT)
        .fetch_all(&self.pool)
        .await?;
        let mut orphans = Vec::with_capacity(inflight_rows.len());
        for row in inflight_rows {
            let payload: String = row.try_get("payload")?;
            match serde_json::from_str::<DistJob>(&payload) {
                Ok(job) => orphans.push(job),
                Err(e) => {
                    warn!(error = %e, "dist_jobs inflight 行 payload 解析失败，跳过");
                }
            }
        }

        Ok(RestoredJobs { queued, orphans })
    }

    /// 读取一行的状态——测试用 / 调试用 / 将来 `fuxi dist inspect` CLI 用。
    pub async fn job_row(&self, job_id: &str) -> Result<Option<JobRow>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, state, assignee, enqueued_at, dispatched_at, completed_at, ok \
             FROM dist_jobs WHERE id = ?1",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(JobRow {
            id: row.try_get("id")?,
            state: row.try_get("state")?,
            assignee: row.try_get("assignee")?,
            enqueued_at: row.try_get("enqueued_at")?,
            dispatched_at: row.try_get("dispatched_at")?,
            completed_at: row.try_get("completed_at")?,
            ok: row.try_get("ok")?,
        }))
    }

    /// 当前 dist_jobs 行总数——并发 enqueue 一致性测试用。
    pub async fn count(&self) -> Result<i64, sqlx::Error> {
        let row = sqlx::query("SELECT COUNT(*) AS c FROM dist_jobs")
            .fetch_one(&self.pool)
            .await?;
        let c: i64 = row.try_get("c")?;
        Ok(c)
    }
}

/// 单行投影——`job_row` 返回。
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: String,
    pub state: String,
    pub assignee: Option<String>,
    pub enqueued_at: String,
    pub dispatched_at: Option<String>,
    pub completed_at: Option<String>,
    pub ok: Option<i64>,
}

impl JobRow {
    /// enqueued_at 的 RFC3339 解析——给 ordering 测试用。
    pub fn enqueued_at_dt(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.enqueued_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }
}

async fn with_busy_retry<F, Fut, T>(mut op: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    let mut attempt: u32 = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(sqlx::Error::Database(db)) if is_busy(db.as_ref()) && attempt < 3 => {
                warn!(attempt, "dist_jobs SQLite BUSY，重试");
                attempt = attempt.saturating_add(1);
                tokio::time::sleep(Duration::from_millis(50_u64.saturating_mul(attempt as u64)))
                    .await;
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_busy(err: &(dyn sqlx::error::DatabaseError + 'static)) -> bool {
    matches!(err.code().as_deref(), Some("5") | Some("6"))
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
    use crate::dist::DistJob;

    fn mk_job(id: &str) -> DistJob {
        DistJob {
            id: id.to_string(),
            node_id: "hint".into(),
            title: format!("title-{id}"),
            body: format!("body-{id}"),
            created_at: 0,
            system_prompt: None,
            required_tags: vec![],
            pinned_node: None,
            cli: String::new(),
            allowed_tools: vec![],
        }
    }

    /// TDD #1：enqueue 写入一行，state='queued'，assignee=NULL，enqueued_at 非空。
    #[tokio::test]
    async fn enqueue_writes_dist_jobs_row_with_state_queued() {
        let p = JobPersistence::connect_memory().await.expect("connect");
        let job = mk_job("j1");
        p.record_enqueue(&job).await.expect("enqueue");

        let row = p.job_row("j1").await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_QUEUED);
        assert!(row.assignee.is_none());
        assert!(!row.enqueued_at.is_empty());
        assert!(row.dispatched_at.is_none());
        assert!(row.completed_at.is_none());
        assert!(row.ok.is_none());
    }

    /// TDD #2：pull 把 state 翻成 'inflight'，assignee=node，dispatched_at 落时间戳。
    #[tokio::test]
    async fn pull_updates_state_to_inflight_with_assignee() {
        let p = JobPersistence::connect_memory().await.expect("connect");
        p.record_enqueue(&mk_job("j2")).await.expect("enqueue");
        p.record_pull("j2", "nodeA").await.expect("pull");

        let row = p.job_row("j2").await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_INFLIGHT);
        assert_eq!(row.assignee.as_deref(), Some("nodeA"));
        assert!(row.dispatched_at.is_some());
        assert!(row.completed_at.is_none());
    }

    /// TDD #3：report 把 state 翻成 'done'，completed_at 落时间戳，ok 写入。
    /// 失败 case 也要测——ok=0 的整数语义不能丢。
    #[tokio::test]
    async fn report_updates_state_to_done_with_ok_flag() {
        let p = JobPersistence::connect_memory().await.expect("connect");
        p.record_enqueue(&mk_job("j_ok")).await.expect("enqueue");
        p.record_pull("j_ok", "nodeA").await.expect("pull");
        p.record_report("j_ok", true).await.expect("report ok");
        let row = p.job_row("j_ok").await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_DONE);
        assert_eq!(row.ok, Some(1));
        assert!(row.completed_at.is_some());
        // assignee 保留——审计能看到是哪个 worker 完成的
        assert_eq!(row.assignee.as_deref(), Some("nodeA"));

        p.record_enqueue(&mk_job("j_fail")).await.expect("enqueue");
        p.record_pull("j_fail", "nodeB").await.expect("pull");
        p.record_report("j_fail", false).await.expect("report fail");
        let row = p.job_row("j_fail").await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_DONE);
        assert_eq!(row.ok, Some(0));
    }

    /// TDD #4：restore 把 'queued' 行还原到 queue；'inflight' 行作为 orphans 给上层处理；
    /// 'done' / 'cancelled' 不出现。
    #[tokio::test]
    async fn restore_from_persistence_repopulates_queue_and_orphans() {
        let p = JobPersistence::connect_memory().await.expect("connect");

        // queued 三个
        p.record_enqueue(&mk_job("q1")).await.expect("e");
        p.record_enqueue(&mk_job("q2")).await.expect("e");
        p.record_enqueue(&mk_job("q3")).await.expect("e");
        // inflight 一个（先 enqueue 再 pull）
        p.record_enqueue(&mk_job("inf1")).await.expect("e");
        p.record_pull("inf1", "dead-node").await.expect("p");
        // done 一个
        p.record_enqueue(&mk_job("d1")).await.expect("e");
        p.record_pull("d1", "live-node").await.expect("p");
        p.record_report("d1", true).await.expect("r");
        // cancelled 一个
        p.record_enqueue(&mk_job("c1")).await.expect("e");
        p.record_cancel("c1").await.expect("c");

        let restored = p.restore().await.expect("restore");
        let queued_ids: Vec<_> = restored.queued.iter().map(|j| j.id.clone()).collect();
        let orphan_ids: Vec<_> = restored.orphans.iter().map(|j| j.id.clone()).collect();

        assert_eq!(
            queued_ids,
            vec!["q1".to_string(), "q2".into(), "q3".into()],
            "queued 应按 enqueued_at 升序，且不含 done/cancelled/inflight"
        );
        assert_eq!(
            orphan_ids,
            vec!["inf1".to_string()],
            "orphans 仅含 inflight 状态的 job"
        );
    }

    /// TDD #5：10 个并发 enqueue，SQLite 行数必须等于 10——并发写一致性 + busy retry 兜底有效。
    #[tokio::test]
    async fn concurrent_enqueue_consistent_persistence() {
        // 用文件库测真并发——`:memory:` max_connections=1 等价串行，测不出 BUSY 路径
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("dist.db");
        let p = std::sync::Arc::new(JobPersistence::connect_file(&path).await.expect("connect"));

        let mut handles = Vec::new();
        for i in 0..10 {
            let p = p.clone();
            handles.push(tokio::spawn(async move {
                let job = mk_job(&format!("c-{i}"));
                p.record_enqueue(&job).await.expect("enqueue");
            }));
        }
        for h in handles {
            h.await.expect("join");
        }

        let n = p.count().await.expect("count");
        assert_eq!(n, 10, "10 并发 enqueue 应得 10 行，实得 {n}");

        // restore 后 queue 也是 10 个
        let r = p.restore().await.expect("restore");
        assert_eq!(r.queued.len(), 10);
        assert!(r.orphans.is_empty());
    }

    /// 兜底：sweep 把 inflight 行刷回 queued、清空 assignee。
    #[tokio::test]
    async fn sweep_flips_inflight_back_to_queued() {
        let p = JobPersistence::connect_memory().await.expect("connect");
        p.record_enqueue(&mk_job("s1")).await.expect("e");
        p.record_pull("s1", "dead-node").await.expect("p");
        p.record_sweep_to_queued("s1").await.expect("sweep");
        let row = p.job_row("s1").await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_QUEUED);
        assert!(row.assignee.is_none());
        assert!(row.dispatched_at.is_none());
    }
}
