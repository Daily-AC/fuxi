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

    /// 拿某 task 的完整历史事件（按 rowid 升序）。
    ///
    /// 给 Extractor（M2.5）用：task 结束后拼 transcript 要按 task_id 取
    /// UserPrompted + AgentResponded。用非流式 `Vec` 而非 stream：任务级事件
    /// 量 O(几十)，接收方直接过滤 enum variant 更顺手，没必要 Stream。
    pub async fn history_for_task(&self, task: fuxi_core::TaskId) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT payload FROM events WHERE task = ?1 ORDER BY rowid ASC")
            .bind(task.to_string())
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let payload: String = row.try_get("payload")?;
            let ev: Event = serde_json::from_str(&payload)?;
            out.push(ev);
        }
        Ok(out)
    }

    /// 列出 events 表里 distinct 的 task id（全部历史）—— 给 IM 后端 `/api/tasks`
    /// 重建 task 列表用。返回顺序：按"该 task 最早事件"的 rowid 升序，让玄女
    /// 派发顺序天然成为 task 列表顺序。
    ///
    /// 该接口拉的是 task id（uuid 字符串），handler 自行 parse + 调
    /// `history_for_task` 拿每条 task 的事件做语义聚合。
    pub async fn list_task_ids(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT task FROM events WHERE task IS NOT NULL \
             GROUP BY task ORDER BY MIN(rowid) ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let task: String = row.try_get("task")?;
            out.push(task);
        }
        Ok(out)
    }

    /// 取最近 `limit` 条事件，按 `rowid` 倒序（最新在前）。
    ///
    /// 给 `fuxi events`（M3.7）救急 CLI 用——绕开 daemon 直读 SQLite 看尾巴。
    /// WHY 倒序而非升序：救急场景关心"刚发生了什么"，按 rowid DESC 取头 N 条
    /// 即可，避免先 count 再 offset 的两步 query。调用方需要时间正序展示
    /// 自行 reverse vec。
    pub async fn recent(&self, limit: i64) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT payload FROM events ORDER BY rowid DESC LIMIT ?1")
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let payload: String = row.try_get("payload")?;
            let ev: Event = serde_json::from_str(&payload)?;
            out.push(ev);
        }
        Ok(out)
    }

    /// 拿全部 `kind_tag == kind` 的事件（按 rowid 升序）。
    ///
    /// `/api/tasks` role 兜底用：旧 task 的 `AgentSpawning` 事件可能远在 history
    /// 尾部之外，用 `recent(N)` 窗口扫不到导致 role 显 "unknown"。这里用 kind_tag
    /// 索引直接命中目标变体，O(spawning 事件数) 远小于 O(全表)。
    pub async fn events_by_kind(&self, kind: &str) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT payload FROM events WHERE kind_tag = ?1 ORDER BY rowid ASC")
            .bind(kind)
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let payload: String = row.try_get("payload")?;
            let ev: Event = serde_json::from_str(&payload)?;
            out.push(ev);
        }
        Ok(out)
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
        TaskBlocked { .. } => "task_blocked",
        TaskResumed { .. } => "task_resumed",
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
        TriggerRegistered { .. } => "trigger_registered",
        TriggerFired { .. } => "trigger_fired",
        TriggerDispatched { .. } => "trigger_dispatched",
        TriggerSkipped { .. } => "trigger_skipped",
        TriggerFailed { .. } => "trigger_failed",
        PlatformStarted { .. } => "platform_started",
        PlatformStopping => "platform_stopping",
        SkillStaged { .. } => "skill_staged",
        SkillApproved { .. } => "skill_approved",
        SkillRejected { .. } => "skill_rejected",
        SkillActivated { .. } => "skill_activated",
        NoRoleMatched { .. } => "no_role_matched",
        AgentRequestReview { .. } => "agent_request_review",
        ReviewRequestTimeout { .. } => "review_request_timeout",
        AgentMessageQueued { .. } => "agent_message_queued",
        AgentMessageDelivered { .. } => "agent_message_delivered",
        AgentMessageRead { .. } => "agent_message_read",
        AgentMessageFailed { .. } => "agent_message_failed",
        WorkerRegistered { .. } => "worker_registered",
        WorkerHeartbeatStateChanged { .. } => "worker_heartbeat_state_changed",
        WorkerStaleSwept { .. } => "worker_stale_swept",
        WorkspaceCreated { .. } => "workspace_created",
        WorkspaceMutated { .. } => "workspace_mutated",
        WorkspaceCommitted { .. } => "workspace_committed",
        WorkspaceArchived { .. } => "workspace_archived",
        WorkspaceCollected { .. } => "workspace_collected",
        WorkspaceQuotaExceeded { .. } => "workspace_quota_exceeded",
        WorkspacePromoted { .. } => "workspace_promoted",
        DeliverableProduced { .. } => "deliverable_produced",
        DeliverableAccepted { .. } => "deliverable_accepted",
        DeliverableRejected { .. } => "deliverable_rejected",
        DeliverableExpired { .. } => "deliverable_expired",
        AgentInlineMessagePushed { .. } => "agent_inline_message_pushed",
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

    /// `recent(N)` 取尾 N 条按 rowid 倒序——最新在前；超过总数返回全部不报错。
    /// `fuxi events --tail N`（M3.7）的查询路径。
    #[tokio::test]
    async fn events_recent_returns_chronological_desc() {
        let store = EventStore::connect_memory().await.expect("connect");
        let a = mk_event("a");
        let b = mk_event("b");
        let c = mk_event("c");
        store.append(&a).await.unwrap();
        store.append(&b).await.unwrap();
        store.append(&c).await.unwrap();

        // tail 2 → 最新两条 c, b（DESC）
        let got = store.recent(2).await.expect("recent");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].meta.id, c.meta.id, "最新在前");
        assert_eq!(got[1].meta.id, b.meta.id);

        // 超出总数：返回全部，不报错
        let all = store.recent(100).await.expect("recent");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].meta.id, c.meta.id);
        assert_eq!(all[2].meta.id, a.meta.id);
    }

    #[tokio::test]
    async fn history_for_task_returns_only_matching_task_in_order() {
        // WHY：Extractor（M2.5）要按 task_id 拉该任务的完整事件序列拼 transcript。
        // 现在没有任何 bus/store API 暴露"按 task 筛"的能力，必须显式返回按存储
        // 顺序升序、过滤同一 task_id 的历史事件。别的 task 的事件不应混进来。
        let store = EventStore::connect_memory().await.expect("connect");
        let t1 = TaskId::new();
        let t2 = TaskId::new();

        let ev_for_task = |task: TaskId, label: &str| {
            let mut meta = EventMeta::now();
            meta.task = Some(task);
            Event {
                meta,
                kind: EventKind::Custom {
                    label: label.to_string(),
                    payload: serde_json::json!({}),
                },
            }
        };
        let a = ev_for_task(t1, "a");
        let b = ev_for_task(t2, "b"); // 噪声，别让它混进来
        let c = ev_for_task(t1, "c");
        for ev in [&a, &b, &c] {
            store.append(ev).await.expect("append");
        }

        let hist = store.history_for_task(t1).await.expect("history");
        assert_eq!(hist.len(), 2, "只应返回 task=t1 的两条事件");
        assert_eq!(hist[0].meta.id, a.meta.id);
        assert_eq!(hist[1].meta.id, c.meta.id);
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

    /// 更漏五个变体的 kind_tag + SQLite roundtrip。
    /// 加 EventKind 变体时必须同步更 kind_tag；此测试做门禁（公理 #M1.3）。
    #[tokio::test]
    async fn persists_trigger_lifecycle_variants() {
        let store = EventStore::connect_memory().await.expect("connect");
        let tid = "trg_test_1".to_string();
        let registered = Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerRegistered {
                id: tid.clone(),
                kind: "cron".into(),
                spec: serde_json::json!({"expr":"*/5 * * * *"}),
            },
        };
        let fired = Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFired {
                id: tid.clone(),
                fired_at: Utc::now(),
                cause: "scheduled".into(),
            },
        };
        let dispatched = Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerDispatched {
                id: tid.clone(),
                to_agent: AgentId::new(),
            },
        };
        let skipped = Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerSkipped {
                id: tid.clone(),
                reason: "overlap".into(),
            },
        };
        let failed = Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFailed {
                id: tid.clone(),
                error: "spawn failed".into(),
            },
        };
        for ev in [&registered, &fired, &dispatched, &skipped, &failed] {
            store.append(ev).await.expect("append");
        }
        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(
            tags,
            vec![
                "trigger_registered",
                "trigger_fired",
                "trigger_dispatched",
                "trigger_skipped",
                "trigger_failed",
            ]
        );
    }

    /// Decision 13 基础：B1 deliverable 边界 nudge 两变体的 kind_tag + SQLite roundtrip。
    /// 加 EventKind 变体时必须同步更 kind_tag；此测试做门禁。
    #[tokio::test]
    async fn persists_deliverable_boundary_variants() {
        use fuxi_core::DeliverableKind;
        let store = EventStore::connect_memory().await.expect("connect");
        let agent = AgentId::new();
        let task = TaskId::new();
        let review = Event {
            meta: EventMeta::now(),
            kind: EventKind::AgentRequestReview {
                agent,
                task,
                deliverable_kind: DeliverableKind::ResearchSummary,
                summary: "调研完成：3 种方案对比".into(),
                artifact_ref: None,
            },
        };
        let original_id = review.meta.id;
        let timeout = Event {
            meta: EventMeta::now(),
            kind: EventKind::ReviewRequestTimeout {
                original_event_id: original_id,
                agent,
                task,
                waited_for_ms: 60_000,
            },
        };
        store.append(&review).await.expect("append review");
        store.append(&timeout).await.expect("append timeout");

        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(tags, vec!["agent_request_review", "review_request_timeout"]);

        // payload 字段保真——尤其 deliverable_kind 的 snake_case 标签和 waited_for_ms 整数。
        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        assert_eq!(got.len(), 2);
        match &got[0].kind {
            EventKind::AgentRequestReview {
                deliverable_kind,
                summary,
                artifact_ref,
                ..
            } => {
                assert_eq!(*deliverable_kind, DeliverableKind::ResearchSummary);
                assert_eq!(summary, "调研完成：3 种方案对比");
                assert!(artifact_ref.is_none());
            }
            other => panic!("expect AgentRequestReview，得到 {other:?}"),
        }
        match &got[1].kind {
            EventKind::ReviewRequestTimeout {
                original_event_id,
                waited_for_ms,
                ..
            } => {
                assert_eq!(*original_event_id, original_id);
                assert_eq!(*waited_for_ms, 60_000);
            }
            other => panic!("expect ReviewRequestTimeout，得到 {other:?}"),
        }
    }

    /// task-scoped mailbox 四变体的 kind_tag + SQLite roundtrip。
    /// 这是门客通信审计链的最低门禁：Queued/Delivered/Read/Failed 都必须可回放。
    #[tokio::test]
    async fn persists_agent_mailbox_variants() {
        let store = EventStore::connect_memory().await.expect("connect");
        let from = AgentId::new();
        let to = AgentId::new();
        let message_id = uuid::Uuid::new_v4();
        let events = vec![
            Event {
                meta: EventMeta::now(),
                kind: EventKind::AgentMessageQueued {
                    message_id,
                    from,
                    to,
                    text: "review diff".into(),
                    summary: Some("review".into()),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::AgentMessageDelivered {
                    message_id,
                    from,
                    to,
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::AgentMessageRead {
                    message_id,
                    reader: to,
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::AgentMessageFailed {
                    message_id,
                    from,
                    to,
                    error: "receiver gone".into(),
                },
            },
        ];
        for ev in &events {
            store.append(ev).await.expect("append");
        }

        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(
            tags,
            vec![
                "agent_message_queued",
                "agent_message_delivered",
                "agent_message_read",
                "agent_message_failed",
            ]
        );

        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        assert_eq!(got.len(), 4);
        match &got[0].kind {
            EventKind::AgentMessageQueued { text, summary, .. } => {
                assert_eq!(text, "review diff");
                assert_eq!(summary.as_deref(), Some("review"));
            }
            other => panic!("expect AgentMessageQueued，得到 {other:?}"),
        }
        match &got[3].kind {
            EventKind::AgentMessageFailed { error, .. } => {
                assert_eq!(error, "receiver gone");
            }
            other => panic!("expect AgentMessageFailed，得到 {other:?}"),
        }
    }

    /// P6 拓扑事件三变体的 kind_tag + SQLite roundtrip + 字段保真。
    /// 加 EventKind 变体时必须同步更 6 处；此测试做门禁。
    #[tokio::test]
    async fn persists_worker_topology_variants() {
        let store = EventStore::connect_memory().await.expect("connect");
        let registered = Event {
            meta: EventMeta::now(),
            kind: EventKind::WorkerRegistered {
                node_id: "nodeA".into(),
                tags: vec!["cc".into(), "gpu".into()],
                max_concurrency: 3,
            },
        };
        let hb = Event {
            meta: EventMeta::now(),
            kind: EventKind::WorkerHeartbeatStateChanged {
                node_id: "nodeA".into(),
                inflight_count: 2,
                status: fuxi_core::WorkerStatus::Alive,
            },
        };
        let swept = Event {
            meta: EventMeta::now(),
            kind: EventKind::WorkerStaleSwept {
                node_id: "nodeA".into(),
                recycled_jobs: vec!["job-1".into(), "job-2".into()],
            },
        };
        for ev in [&registered, &hb, &swept] {
            store.append(ev).await.expect("append");
        }
        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(
            tags,
            vec![
                "worker_registered",
                "worker_heartbeat_state_changed",
                "worker_stale_swept",
            ]
        );

        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        assert_eq!(got.len(), 3);
        match &got[0].kind {
            EventKind::WorkerRegistered {
                node_id,
                tags,
                max_concurrency,
            } => {
                assert_eq!(node_id, "nodeA");
                assert_eq!(tags, &vec!["cc".to_string(), "gpu".to_string()]);
                assert_eq!(*max_concurrency, 3);
            }
            other => panic!("expect WorkerRegistered，得到 {other:?}"),
        }
        match &got[1].kind {
            EventKind::WorkerHeartbeatStateChanged {
                node_id,
                inflight_count,
                status,
            } => {
                assert_eq!(node_id, "nodeA");
                assert_eq!(*inflight_count, 2);
                assert_eq!(*status, fuxi_core::WorkerStatus::Alive);
            }
            other => panic!("expect WorkerHeartbeatStateChanged，得到 {other:?}"),
        }
        match &got[2].kind {
            EventKind::WorkerStaleSwept {
                node_id,
                recycled_jobs,
            } => {
                assert_eq!(node_id, "nodeA");
                assert_eq!(
                    recycled_jobs,
                    &vec!["job-1".to_string(), "job-2".to_string()]
                );
            }
            other => panic!("expect WorkerStaleSwept，得到 {other:?}"),
        }
    }

    /// Decision 21 phase 1：workspace 七变体 kind_tag + SQLite roundtrip + 字段保真。
    /// 加 EventKind 变体时必须同步五处；此测试做门禁。
    #[tokio::test]
    async fn persists_workspace_lifecycle_variants() {
        use fuxi_core::{ArchiveReason, ProjectId, QuotaKind, WorkspaceId, WorkspaceLayer};
        use std::path::PathBuf;
        let store = EventStore::connect_memory().await.expect("connect");
        let pid = ProjectId::new("erp").unwrap();
        let task = TaskId::new();
        let ws_l3 = WorkspaceId::l3(&pid, "luban");
        let ws_l2 = WorkspaceId::l2(&pid, task);

        let events = vec![
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceCreated {
                    workspace_id: ws_l3.clone(),
                    project: pid.clone(),
                    layer: WorkspaceLayer::L3Persistent,
                    role: Some("luban".into()),
                    task: None,
                    path: PathBuf::from("/tmp/erp/sandboxes/luban"),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceMutated {
                    workspace_id: ws_l3.clone(),
                    files_changed: 5,
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceCommitted {
                    workspace_id: ws_l3.clone(),
                    commit_sha: "abc1234567890".into(),
                    branch: "luban/erp-main".into(),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceArchived {
                    workspace_id: ws_l2.clone(),
                    reason: ArchiveReason::TaskCompleted,
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceCollected {
                    workspace_id: ws_l2.clone(),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspaceQuotaExceeded {
                    project: pid.clone(),
                    quota_kind: QuotaKind::DiskBytes,
                    requested: 6_000_000_000,
                    limit: 5_000_000_000,
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkspacePromoted {
                    from_workspace_id: ws_l2.clone(),
                    to_role: "luban".into(),
                    project: pid.clone(),
                },
            },
        ];
        for ev in &events {
            store.append(ev).await.expect("append");
        }

        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(
            tags,
            vec![
                "workspace_created",
                "workspace_mutated",
                "workspace_committed",
                "workspace_archived",
                "workspace_collected",
                "workspace_quota_exceeded",
                "workspace_promoted",
            ]
        );

        // 字段保真：抽样校 path / branch / quota_kind 三处
        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        match &got[0].kind {
            EventKind::WorkspaceCreated { path, layer, .. } => {
                assert_eq!(path, &PathBuf::from("/tmp/erp/sandboxes/luban"));
                assert_eq!(*layer, WorkspaceLayer::L3Persistent);
            }
            other => panic!("expect WorkspaceCreated，得到 {other:?}"),
        }
        match &got[2].kind {
            EventKind::WorkspaceCommitted { branch, .. } => assert_eq!(branch, "luban/erp-main"),
            other => panic!("expect WorkspaceCommitted，得到 {other:?}"),
        }
        match &got[5].kind {
            EventKind::WorkspaceQuotaExceeded {
                quota_kind,
                requested,
                limit,
                ..
            } => {
                assert_eq!(*quota_kind, QuotaKind::DiskBytes);
                assert_eq!(*requested, 6_000_000_000);
                assert_eq!(*limit, 5_000_000_000);
            }
            other => panic!("expect WorkspaceQuotaExceeded，得到 {other:?}"),
        }
    }

    /// Decision 22：deliverable 四变体 kind_tag + SQLite roundtrip + 字段保真。
    #[tokio::test]
    async fn persists_deliverable_storage_variants() {
        use fuxi_core::{DeliverableFileMeta, DeliverableKind, ProjectId};
        use std::path::PathBuf;
        let store = EventStore::connect_memory().await.expect("connect");
        let task = TaskId::new();
        let pid = ProjectId::new("erp").unwrap();

        let events = vec![
            Event {
                meta: EventMeta::now(),
                kind: EventKind::DeliverableProduced {
                    task,
                    project: pid.clone(),
                    deliverable_kind: DeliverableKind::ResearchSummary,
                    files: vec![DeliverableFileMeta {
                        name: "report.md".into(),
                        sha256: "abc123def456".into(),
                        size_bytes: 4096,
                    }],
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::DeliverableAccepted {
                    task,
                    accepted_to: Some(PathBuf::from("/Users/e0_7/写作/2026.md")),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::DeliverableRejected {
                    task,
                    reason: Some("内容不对题".into()),
                },
            },
            Event {
                meta: EventMeta::now(),
                kind: EventKind::DeliverableExpired { task },
            },
        ];
        for ev in &events {
            store.append(ev).await.expect("append");
        }

        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(
            tags,
            vec![
                "deliverable_produced",
                "deliverable_accepted",
                "deliverable_rejected",
                "deliverable_expired",
            ]
        );

        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        match &got[0].kind {
            EventKind::DeliverableProduced {
                deliverable_kind,
                files,
                ..
            } => {
                assert_eq!(*deliverable_kind, DeliverableKind::ResearchSummary);
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].name, "report.md");
                assert_eq!(files[0].size_bytes, 4096);
            }
            other => panic!("expect DeliverableProduced，得到 {other:?}"),
        }
        match &got[1].kind {
            EventKind::DeliverableAccepted { accepted_to, .. } => {
                assert_eq!(
                    accepted_to
                        .as_deref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    Some("/Users/e0_7/写作/2026.md".to_string())
                );
            }
            other => panic!("expect DeliverableAccepted，得到 {other:?}"),
        }
    }

    /// δ #4 回归保护：`EventMeta.source_node_id` 必须经 SQLite payload blob
    /// 完整 roundtrip——v1 不加专列存它，全靠 `serde_json::to_string(ev)` 把
    /// 它落进 payload。若哪天有人为了"性能"把 payload 改成只存 EventKind，本测会挂。
    #[tokio::test]
    async fn persists_event_meta_source_node_id_in_payload_blob() {
        let store = EventStore::connect_memory().await.expect("connect");
        let mut meta = fuxi_core::EventMeta::now();
        meta.source_node_id = Some("far".into());
        let ev = Event {
            meta,
            kind: EventKind::AgentResponded {
                text: "远端响应".into(),
            },
        };
        store.append(&ev).await.expect("append");

        let got = collect_ok(store.replay(ReplayCursor::Beginning))
            .await
            .expect("replay");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].meta.source_node_id.as_deref(), Some("far"));
        match &got[0].kind {
            EventKind::AgentResponded { text } => assert_eq!(text, "远端响应"),
            other => panic!("expect AgentResponded, got {other:?}"),
        }
    }

    /// M3.6 回归保护：删掉 TaskDelivered/TaskCancelled 变体后，task 终态
    /// **必须**仍能通过 TaskStateChanged{Done|Cancelled} 落盘+回放并保留字段。
    /// 这是替代两个孤儿变体的**唯一通道**，丢失这条线 = M3.6 走偏。
    #[tokio::test]
    async fn task_terminal_roundtrip_via_state_changed_only() {
        use fuxi_core::task::TaskState;

        let store = EventStore::connect_memory().await.expect("connect");
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        let done = Event {
            meta: meta.clone(),
            kind: EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        };
        let mut meta2 = EventMeta::now();
        meta2.task = Some(task);
        let cancelled = Event {
            meta: meta2,
            kind: EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Cancelled,
            },
        };
        store.append(&done).await.expect("append done");
        store.append(&cancelled).await.expect("append cancelled");

        let hist = store.history_for_task(task).await.expect("history");
        assert_eq!(hist.len(), 2);
        match &hist[0].kind {
            EventKind::TaskStateChanged { to, .. } => assert_eq!(*to, TaskState::Done),
            other => panic!("expect TaskStateChanged Done，得到 {other:?}"),
        }
        match &hist[1].kind {
            EventKind::TaskStateChanged { to, .. } => assert_eq!(*to, TaskState::Cancelled),
            other => panic!("expect TaskStateChanged Cancelled，得到 {other:?}"),
        }

        // kind_tag 必须只有 task_state_changed——不再有 task_delivered/task_cancelled。
        let tags: Vec<String> = sqlx::query("SELECT kind_tag FROM events ORDER BY rowid ASC")
            .fetch_all(store.pool())
            .await
            .expect("fetch")
            .into_iter()
            .map(|r| r.try_get::<String, _>("kind_tag").expect("kind_tag"))
            .collect();
        assert_eq!(tags, vec!["task_state_changed", "task_state_changed"]);
    }
}
