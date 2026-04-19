//! 伏羲 EventStore —— 事件的持久化与回放。
//!
//! 公理 #5 要求 SQLite 是单一真相源；这里负责把 `Event` 写进
//! `events` 表、并按游标（id / 时间 / 起点）流式回放历史。
//!
//! 为什么把 schema 内嵌在源码里：sqlx 的 `migrate!()` 需要编译期路径，
//! 而该 crate 会被嵌入到多个二进制里运行——把 SQL 字符串直接 `include_str!`
//! 进来可以保证 `connect(":memory:")` 这种临时库也能跑起来。

use crate::Result;
use chrono::{DateTime, Utc};
use futures_util::stream::Stream;
use fuxi_core::Event;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, warn};
use uuid::Uuid;

/// `migrations/0001_init.sql` 的内嵌副本。编译期嵌入避免运行期路径依赖。
const SCHEMA_SQL: &str = include_str!("../migrations/0001_init.sql");

/// 回放游标：从头、从给定 id 之后（不含该条）、或从给定时刻起。
#[derive(Debug, Clone)]
pub enum ReplayCursor {
    /// 从最早一条事件开始。
    Beginning,
    /// 从 id 为 `Uuid` 的事件**之后**开始（严格大于其存储顺序）。
    FromId(Uuid),
    /// 从给定 UTC 时间起（含该时间点上的事件）。
    FromTime(DateTime<Utc>),
}

/// 事件存储。内部是 `SqlitePool`，clone 便宜（`Arc` 语义）。
#[derive(Debug, Clone)]
pub struct EventStore {
    pool: SqlitePool,
}

/// 回放流类型别名——跨模块签名稳定。
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event>> + Send + 'static>>;

impl EventStore {
    /// 连接到 `:memory:` 的临时库。测试首选。
    pub async fn connect_memory() -> Result<Self> {
        // 为什么限制最大连接数为 1：SQLite `:memory:` 每条连接是独立库，
        // 多连接时测试里写入后读不到。单连接避免这个坑。
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str("sqlite::memory:")?)
            .await?;
        let store = Self { pool };
        store.init_schema().await?;
        Ok(store)
    }

    /// 连接到文件库并自动初始化 schema + 打开 WAL。
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
        // SQLite 里 `CREATE TABLE IF NOT EXISTS` + `CREATE INDEX IF NOT EXISTS` 幂等安全。
        for stmt in split_sql(SCHEMA_SQL) {
            if stmt.trim().is_empty() {
                continue;
            }
            sqlx::query(&stmt).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// 写入一条事件。Append-only，不 UPDATE。
    pub async fn append(&self, ev: &Event) -> Result<()> {
        let id = ev.meta.id.to_string();
        let at = ev.meta.at.to_rfc3339();
        let session = ev.meta.session.map(|s| s.to_string());
        let agent = ev.meta.agent.map(|a| a.to_string());
        let task = ev.meta.task.map(|t| t.to_string());
        let kind_tag = kind_tag(&ev.kind);
        let payload = serde_json::to_string(ev)?;

        // WAL 下偶发 BUSY——sqlx 的 busy_timeout 已处理绝大多数；这里再兜三次指数退避，
        // 避免偶发错误把 writer 任务打挂。
        let mut attempt: u32 = 0;
        loop {
            let res = sqlx::query(
                "INSERT INTO events (id, at, session, agent, task, kind_tag, payload) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&id)
            .bind(&at)
            .bind(&session)
            .bind(&agent)
            .bind(&task)
            .bind(kind_tag)
            .bind(&payload)
            .execute(&self.pool)
            .await;

            match res {
                Ok(_) => {
                    debug!(event_id = %id, kind = kind_tag, "事件已入库");
                    return Ok(());
                }
                Err(sqlx::Error::Database(db)) if is_busy(db.as_ref()) && attempt < 3 => {
                    warn!(attempt, "SQLite BUSY，重试 append");
                    attempt = attempt.saturating_add(1);
                    tokio::time::sleep(Duration::from_millis(
                        50_u64.saturating_mul(attempt as u64),
                    ))
                    .await;
                }
                Err(e) => {
                    error!(event_id = %id, error = %e, "事件写入失败");
                    return Err(e.into());
                }
            }
        }
    }

    /// 从游标起流式回放历史事件。顺序：`recorded_at` 升序，平局时 `id` 升序。
    pub fn replay(&self, cursor: ReplayCursor) -> EventStream {
        // 为什么用 channel 而非 async_stream：当前 crate 依赖集合不含 async-stream；
        // 手写 producer task 也能让错误边界、生命周期都更显式。
        let pool = self.pool.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event>>(64);
        tokio::spawn(async move {
            if let Err(e) = drain_cursor(&pool, cursor, &tx).await {
                // 把底层错误送到下游一次——下游 consumer 可以决定是否提前终止。
                let _ = tx.send(Err(e)).await;
            }
        });
        Box::pin(ReceiverStream::new(rx))
    }

    /// 同 crate 内暴露 pool 给 bus/测试使用。
    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

async fn drain_cursor(
    pool: &SqlitePool,
    cursor: ReplayCursor,
    tx: &tokio::sync::mpsc::Sender<Result<Event>>,
) -> Result<()> {
    match cursor {
        ReplayCursor::Beginning => {
            let rows = sqlx::query("SELECT payload FROM events ORDER BY rowid ASC")
                .fetch_all(pool)
                .await?;
            for row in rows {
                let payload: String = row.try_get("payload")?;
                let ev: Event = serde_json::from_str(&payload)?;
                if tx.send(Ok(ev)).await.is_err() {
                    return Ok(());
                }
            }
        }
        ReplayCursor::FromId(uuid) => {
            // 用 anchor 行的 rowid 定位“之后”的集合。
            // 为什么用 rowid 而非 uuid 自身：Uuid v4 无顺序语义，rowid 单调递增且稳定。
            let anchor = sqlx::query("SELECT rowid FROM events WHERE id = ?1")
                .bind(uuid.to_string())
                .fetch_optional(pool)
                .await?;

            let Some(row) = anchor else {
                // 锚点不存在：返回空流（约定）。
                return Ok(());
            };
            let anchor_rowid: i64 = row.try_get("rowid")?;

            let rows =
                sqlx::query("SELECT payload FROM events WHERE rowid > ?1 ORDER BY rowid ASC")
                    .bind(anchor_rowid)
                    .fetch_all(pool)
                    .await?;
            for row in rows {
                let payload: String = row.try_get("payload")?;
                let ev: Event = serde_json::from_str(&payload)?;
                if tx.send(Ok(ev)).await.is_err() {
                    return Ok(());
                }
            }
        }
        ReplayCursor::FromTime(ts) => {
            let ts_s = ts.to_rfc3339();
            let rows = sqlx::query(
                "SELECT payload FROM events WHERE at >= ?1 \
                 ORDER BY at ASC, id ASC",
            )
            .bind(&ts_s)
            .fetch_all(pool)
            .await?;
            for row in rows {
                let payload: String = row.try_get("payload")?;
                let ev: Event = serde_json::from_str(&payload)?;
                if tx.send(Ok(ev)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// 判定 sqlite 错误码是否 BUSY/LOCKED。
fn is_busy(err: &(dyn sqlx::error::DatabaseError + 'static)) -> bool {
    matches!(err.code().as_deref(), Some("5") | Some("6"))
}

/// 取 `EventKind` 的 serde "type" tag（字符串形式）。
/// 为什么写死 match 而不用 serde_json 动态取：返回 `&'static str` 避免临时分配；
/// 编译期对新增变体会警告 `non_exhaustive`（本枚举非 `#[non_exhaustive]`，缺失变体会编译失败）。
fn kind_tag(kind: &fuxi_core::EventKind) -> &'static str {
    use fuxi_core::EventKind::*;
    match kind {
        AgentSpawning { .. } => "agent_spawning",
        AgentReady { .. } => "agent_ready",
        AgentShuttingDown { .. } => "agent_shutting_down",
        AgentDead { .. } => "agent_dead",
        TaskCreated { .. } => "task_created",
        TaskDispatched { .. } => "task_dispatched",
        TaskStateChanged { .. } => "task_state_changed",
        TaskDelivered { .. } => "task_delivered",
        TaskBlocked { .. } => "task_blocked",
        TaskResumed { .. } => "task_resumed",
        TaskCancelled { .. } => "task_cancelled",
        MessageSent { .. } => "message_sent",
        MessageReceived { .. } => "message_received",
        UserPrompted { .. } => "user_prompted",
        AgentResponded { .. } => "agent_responded",
        ThinkingStarted => "thinking_started",
        ThinkingFinished => "thinking_finished",
        ToolCallStarted { .. } => "tool_call_started",
        ToolCallFinished { .. } => "tool_call_finished",
        UserInterventionSent { .. } => "user_intervention_sent",
        AgentInterrupted { .. } => "agent_interrupted",
        TaskInterventionApplied { .. } => "task_intervention_applied",
        OrchestratorCcReceived { .. } => "orchestrator_cc_received",
        ConversationTransferred { .. } => "conversation_transferred",
        ConversationReturned { .. } => "conversation_returned",
        PlatformStarted { .. } => "platform_started",
        PlatformStopping => "platform_stopping",
        SkillStaged { .. } => "skill_staged",
        SkillApproved { .. } => "skill_approved",
        SkillRejected { .. } => "skill_rejected",
        SkillActivated { .. } => "skill_activated",
        NoRoleMatched { .. } => "no_role_matched",
        Custom { .. } => "custom",
    }
}

/// 粗略切分：按 `;` 分，忽略注释行（`--` 开头）。对手写 migration 来说够用。
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
    use crate::Error as EventsError;
    use futures_util::StreamExt;
    use fuxi_core::{AgentId, EventKind, EventMeta, TaskId};
    use tokio::time::{Duration, sleep};

    fn mk_event(label: &str) -> Event {
        Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: label.to_string(),
                payload: serde_json::json!({"l": label}),
            },
        }
    }

    fn mk_agent_event(agent: AgentId, task: TaskId) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        meta.task = Some(task);
        Event {
            meta,
            kind: EventKind::TaskCreated {
                title: "t".into(),
                description: "d".into(),
            },
        }
    }

    async fn collect_ok(mut s: EventStream) -> std::result::Result<Vec<Event>, EventsError> {
        let mut out = Vec::new();
        while let Some(item) = s.next().await {
            out.push(item?);
        }
        Ok(out)
    }

    #[tokio::test]
    async fn roundtrip_beginning() {
        let store = EventStore::connect_memory().await.expect("connect");
        let a = mk_event("a");
        let b = mk_event("b");
        store.append(&a).await.expect("append a");
        store.append(&b).await.expect("append b");
        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].meta.id, a.meta.id);
        assert_eq!(got[1].meta.id, b.meta.id);
    }

    #[tokio::test]
    async fn replay_from_id_returns_strictly_after() {
        let store = EventStore::connect_memory().await.expect("connect");
        let a = mk_event("a");
        let b = mk_event("b");
        let c = mk_event("c");
        store.append(&a).await.expect("append a");
        store.append(&b).await.expect("append b");
        store.append(&c).await.expect("append c");
        let got = collect_ok(store.replay(ReplayCursor::FromId(a.meta.id)))
            .await
            .expect("replay from a");
        // 严格 *之后* a，包含 b 和 c。
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].meta.id, b.meta.id);
        assert_eq!(got[1].meta.id, c.meta.id);
    }

    #[tokio::test]
    async fn replay_from_time_monotonic() {
        let store = EventStore::connect_memory().await.expect("connect");
        let a = mk_event("a");
        store.append(&a).await.expect("append a");
        // 为什么 sleep：保证 Utc::now() 的 rfc3339 字符串排序严格递增。
        sleep(Duration::from_millis(20)).await;
        let cutoff = Utc::now();
        sleep(Duration::from_millis(20)).await;
        let b = mk_event("b");
        store.append(&b).await.expect("append b");
        let got = collect_ok(store.replay(ReplayCursor::FromTime(cutoff)))
            .await
            .expect("replay from time");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].meta.id, b.meta.id);
    }

    #[tokio::test]
    async fn persists_skill_lifecycle_kind_tags() {
        let store = EventStore::connect_memory().await.expect("connect");
        let events = [
            Event {
                meta: EventMeta::now(),
                kind: EventKind::NoRoleMatched {
                    need: "画图门客".into(),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::SkillStaged {
                    role: "painter".into(),
                    template: "dev".into(),
                    path: "/tmp/painter.staging/SKILL.md".into(),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::SkillApproved {
                    role: "painter".into(),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::SkillRejected {
                    role: "liar".into(),
                    reason: "frontmatter 不合法".into(),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::SkillActivated {
                    role: "painter".into(),
                },
            },
        ];
        for ev in &events {
            store.append(ev).await.expect("append");
        }

        let rows = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch");
        let tags: Vec<String> = rows
            .iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("tag"))
            .collect();
        assert_eq!(
            tags,
            vec![
                "no_role_matched",
                "skill_staged",
                "skill_approved",
                "skill_rejected",
                "skill_activated",
            ]
        );

        // 回放往返：payload 能 deser 成原 enum。
        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        assert_eq!(got.len(), 5);
    }

    #[tokio::test]
    async fn persists_agent_and_task_indices() {
        let store = EventStore::connect_memory().await.expect("connect");
        let agent = AgentId::new();
        let task = TaskId::new();
        store
            .append(&mk_agent_event(agent, task))
            .await
            .expect("append");
        let row = sqlx::query("SELECT agent, task FROM events")
            .fetch_one(store.pool())
            .await
            .expect("fetch");
        let agent_str: String = row.try_get("agent").expect("agent");
        let task_str: String = row.try_get("task").expect("task");
        assert_eq!(agent_str, agent.to_string());
        assert_eq!(task_str, task.to_string());
    }
}
