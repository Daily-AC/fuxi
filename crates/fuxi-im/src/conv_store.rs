//! IM 层聊天记录存储（Task #17）—— `conversations` + `messages` 表 CRUD。
//!
//! 与 fuxi-events 的 task-indexed 审计日志正交：
//! - events 是"事实流水账"——按 task / agent / time 索引
//! - 这里是"对话视图"——按 conversation 索引，前端首屏用
//!
//! ## 后台 sync hook
//!
//! [`spawn_xuannv_sync`] 起一个后台 task 订阅 EventBus，把"玄女主线"事件翻译成
//! messages 行。失败 warn 不阻 EventBus（写盘错绝不能让事件总线崩）。
//!
//! ## 阶段
//!
//! v0.1：UserInterventionSent + AgentResponded + OrchestratorCcReceived + TaskCreated
//! 四类事件；ToolCallStarted/Finished 等到阶段 3。
//! AgentTextDelta 在 EventKind 里没有该名称；当前 cc 流式 token 走的是
//! `AgentResponded { text }` 一次性 final 文本——不需要累积 buffer。
//! 等以后 EventKind 真加增量变体时回来扩。

#![allow(dead_code)]

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use fuxi_core::{AgentId, Event, EventKind};
use fuxi_events::EventBus;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, error, warn};
use uuid::Uuid;

/// 玄女主线 conversation 的 scope 字符串——前端用 `?conv=xuannv` 拉它。
pub const SCOPE_XUANNV: &str = "xuannv";

/// 单条消息——读出来给前端的 wire 形态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conv_id: String,
    pub role: String,
    pub agent_id: Option<String>,
    pub kind: String,
    /// content 是原始 JSON object——前端按 kind 解析。
    pub content: serde_json::Value,
    pub attachments: Option<serde_json::Value>,
    pub source_event_id: Option<String>,
    pub ts: DateTime<Utc>,
    /// v1-session19 #2 · 服务端 hydrate 的附件元数据（id/name/mime/bytes/sha256）。
    /// conv handler 在 `page_messages` 后批量 lookup `upload_store` 填上，让前端
    /// 历史回放显真名而非 uuid。空 = 该消息无附件 *或* 后端未注入 upload_store
    /// （前端会 fallback 老路径用 id 当 name）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_uploads: Vec<crate::uploads::UploadDigest>,
}

/// conversation 行——列表视图用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub scope: String,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub message_count: i64,
}

/// 包装 SqlitePool 提供 conversation/message CRUD。
#[derive(Clone)]
pub struct ConvStore {
    pool: SqlitePool,
}

impl ConvStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 确保给定 scope 的 conversation 存在；不存在则创建。返回 conversation id。
    /// daemon 启动期 + sync hook 第一次写消息前都该调一次（幂等）。
    pub async fn ensure_scope(&self, scope: &str, title: Option<&str>) -> Result<String> {
        // 已存在 → 返已有 id（不动 title）
        if let Some(id) =
            sqlx::query_scalar::<_, String>("SELECT id FROM conversations WHERE scope = ?1")
                .bind(scope)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| Error::Internal(format!("ensure_scope query: {e}")))?
        {
            return Ok(id);
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO conversations (id, scope, title, created_at, last_active_at, message_count) \
             VALUES (?1, ?2, ?3, ?4, ?4, 0)",
        )
        .bind(&id)
        .bind(scope)
        .bind(title)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("ensure_scope insert: {e}")))?;
        Ok(id)
    }

    /// 写一条消息进 conv，**事务里同步** bump conversations.message_count + last_active_at。
    /// 调用方必须先 `ensure_scope` 拿到 conv_id。
    #[allow(clippy::too_many_arguments)]
    pub async fn append_message(
        &self,
        conv_id: &str,
        role: &str,
        agent_id: Option<&str>,
        kind: &str,
        content: &serde_json::Value,
        attachments: Option<&serde_json::Value>,
        source_event_id: Option<&str>,
        ts: DateTime<Utc>,
    ) -> Result<String> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Internal(format!("tx begin: {e}")))?;
        let id = Uuid::new_v4().to_string();
        let ts_str = ts.to_rfc3339();
        let content_str = serde_json::to_string(content)
            .map_err(|e| Error::Internal(format!("content json: {e}")))?;
        let attach_str = attachments
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| Error::Internal(format!("attach json: {e}")))?;

        sqlx::query(
            "INSERT INTO messages (id, conv_id, role, agent_id, kind, content, attachments, \
             source_event_id, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&id)
        .bind(conv_id)
        .bind(role)
        .bind(agent_id)
        .bind(kind)
        .bind(&content_str)
        .bind(attach_str.as_deref())
        .bind(source_event_id)
        .bind(&ts_str)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Internal(format!("messages insert: {e}")))?;

        sqlx::query(
            "UPDATE conversations \
             SET message_count = message_count + 1, last_active_at = ?2 \
             WHERE id = ?1",
        )
        .bind(conv_id)
        .bind(&ts_str)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Internal(format!("conv update: {e}")))?;

        tx.commit()
            .await
            .map_err(|e| Error::Internal(format!("tx commit: {e}")))?;
        Ok(id)
    }

    /// 读 conversation 的消息——按 ts 升序，返回 (messages, has_more, oldest_id_in_page)。
    ///
    /// `before` 是消息 id（uuid）——分页时往前翻历史；`None` = 取最新一页。
    /// `limit` 上限 200；超出 clamp。
    ///
    /// 返回顺序按 ts ASC（前端从最早到最近渲染）。`has_more` 表示当前页之外
    /// **更早**的还有；oldest 是本页最早消息 id（前端下次 `before=oldest`）。
    pub async fn page_messages(
        &self,
        conv_id: &str,
        limit: usize,
        before: Option<&str>,
    ) -> Result<(Vec<Message>, bool, Option<String>)> {
        let limit = limit.clamp(1, 200);
        // 查 limit+1 条来判 has_more
        let fetch_n = limit as i64 + 1;

        // 取最新的"<= 这条"或"< 这条 ts"的消息——按 ts DESC + id DESC tiebreaker，
        // 然后 reverse 给前端 ASC 顺序。before 走 ts 比较（消息 id 是 uuid 无序，
        // 但一行有 ts，自带顺序）。
        let rows = if let Some(before_id) = before {
            // 先查 anchor 行的 ts
            let anchor_ts: Option<String> =
                sqlx::query_scalar("SELECT ts FROM messages WHERE id = ?1 AND conv_id = ?2")
                    .bind(before_id)
                    .bind(conv_id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| Error::Internal(format!("anchor ts: {e}")))?;
            let Some(anchor_ts) = anchor_ts else {
                // anchor 不存在/不属于此 conv → 空页 + has_more=false
                return Ok((Vec::new(), false, None));
            };
            sqlx::query(
                "SELECT id, conv_id, role, agent_id, kind, content, attachments, \
                 source_event_id, ts FROM messages \
                 WHERE conv_id = ?1 AND ts < ?2 \
                 ORDER BY ts DESC, id DESC LIMIT ?3",
            )
            .bind(conv_id)
            .bind(&anchor_ts)
            .bind(fetch_n)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT id, conv_id, role, agent_id, kind, content, attachments, \
                 source_event_id, ts FROM messages \
                 WHERE conv_id = ?1 \
                 ORDER BY ts DESC, id DESC LIMIT ?2",
            )
            .bind(conv_id)
            .bind(fetch_n)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| Error::Internal(format!("page_messages query: {e}")))?;

        let has_more = rows.len() as i64 > limit as i64;
        let take = if has_more { limit } else { rows.len() };
        let mut msgs: Vec<Message> = rows
            .into_iter()
            .take(take)
            .map(row_to_message)
            .collect::<Result<Vec<_>>>()?;
        // DB 给的是 DESC，反过来给前端 ASC 顺序
        msgs.reverse();
        let oldest = msgs.first().map(|m| m.id.clone());
        Ok((msgs, has_more, oldest))
    }

    /// 测试 helper —— 暴露 pool 让测试直接发 SQL 验内部状态。
    #[cfg(test)]
    pub(crate) fn handle(&self) -> &SqlitePool {
        &self.pool
    }
}

fn row_to_message(row: sqlx::sqlite::SqliteRow) -> Result<Message> {
    let content_str: String = row
        .try_get("content")
        .map_err(|e| Error::Internal(format!("row content: {e}")))?;
    let content: serde_json::Value = serde_json::from_str(&content_str)
        .map_err(|e| Error::Internal(format!("content parse: {e}")))?;
    let attach_str: Option<String> = row
        .try_get("attachments")
        .map_err(|e| Error::Internal(format!("row attach: {e}")))?;
    let attachments = attach_str
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| Error::Internal(format!("attach parse: {e}")))?;
    let ts_str: String = row
        .try_get("ts")
        .map_err(|e| Error::Internal(format!("row ts: {e}")))?;
    let ts = DateTime::parse_from_rfc3339(&ts_str)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| Error::Internal(format!("ts parse '{ts_str}': {e}")))?;
    Ok(Message {
        id: row
            .try_get("id")
            .map_err(|e| Error::Internal(format!("row id: {e}")))?,
        conv_id: row
            .try_get("conv_id")
            .map_err(|e| Error::Internal(format!("row conv_id: {e}")))?,
        role: row
            .try_get("role")
            .map_err(|e| Error::Internal(format!("row role: {e}")))?,
        agent_id: row
            .try_get("agent_id")
            .map_err(|e| Error::Internal(format!("row agent_id: {e}")))?,
        kind: row
            .try_get("kind")
            .map_err(|e| Error::Internal(format!("row kind: {e}")))?,
        content,
        attachments,
        source_event_id: row
            .try_get("source_event_id")
            .map_err(|e| Error::Internal(format!("row src: {e}")))?,
        ts,
        // hydrate 由调用方（conv handler）后置填充——这里给空集，保持 store 层
        // 跟 upload_store 解耦
        attachment_uploads: Vec::new(),
    })
}

/// 起后台 sync task：订阅 EventBus，把玄女主线相关事件翻译为 messages 行。
///
/// 返回 JoinHandle，调用方可 abort。
///
/// `xuannv_id_watch`：来自 [`fuxi_orchestrator::Fuxi::xuannv_id_watch`] 的 watch
/// channel——每条事件**实时取最新值**做"我 vs 别人"的过滤。WHY watch 而不是
/// 静态 `AgentId`：handoff 路径会 kill 旧 xuannv 再 spawn 新副本（agent_id 变），
/// 静态 id 会让 sync 用旧 id 死过滤新副本的所有事件，新副本发言全部落不进
/// messages 表，PWA IM 看不到——bug "handoff 后新玄女副本发言不进 PWA IM 历史"。
///
/// **同步期完成**：
/// - 进 conv `ensure_scope`
/// - 进 `bus.subscribe()` 拿到 stream
///
/// 这俩在返回前同步发生——调用方 `spawn_xuannv_sync(...).await` 完毕**那刻起**
/// 任何后续 `bus.publish` 都不会再丢。**不是**先 spawn 再 subscribe（那会 race
/// 掉 spawn 后到 subscribe 之间的事件）。
pub async fn spawn_xuannv_sync(
    store: Arc<ConvStore>,
    bus: EventBus,
    xuannv_id_watch: watch::Receiver<Option<AgentId>>,
) -> tokio::task::JoinHandle<()> {
    // 同步期：先 ensure conv + 先 subscribe
    let conv_id = match store.ensure_scope(SCOPE_XUANNV, None).await {
        Ok(id) => id,
        Err(e) => {
            error!(error = %e, "ensure xuannv conv 失败，sync 不启动");
            return tokio::spawn(async {});
        }
    };
    let mut stream = bus.subscribe();
    debug!(
        conv = %conv_id,
        xuannv = ?*xuannv_id_watch.borrow(),
        "conv_store xuannv sync 准备就绪"
    );

    // 之后才 spawn 长跑 task 消费 stream
    tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            let ev = match item {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "conv_store sync recv 错误，继续");
                    continue;
                }
            };
            // 每条事件 borrow 当前 xuannv id——handoff 切副本后立刻用新值过滤。
            // None = 玄女尚未上线（理论上 spawn 这个 sync 之前 set_xuannv 已调过，
            // 所以此分支极少；保留兜底防 daemon 启动顺序 race）。
            let Some(current_xn) = *xuannv_id_watch.borrow() else {
                continue;
            };
            if let Err(e) = handle_event(&store, &conv_id, current_xn, &ev).await {
                warn!(error = %e, kind = ?ev.kind, "conv_store sync 写库失败");
            }
        }
        debug!("conv_store xuannv sync 退出（bus 流关闭）");
    })
}

/// 把单条 Event 翻成 messages 行（如果该事件该入对话视图）。
async fn handle_event(
    store: &ConvStore,
    conv_id: &str,
    xuannv_id: AgentId,
    ev: &Event,
) -> Result<()> {
    let source_id = ev.meta.id.to_string();
    match &ev.kind {
        // 用户对玄女说话的两种入口都翻 user role：
        // - UserPrompted（玄女当前 turn 的 prompt）
        // - UserInterventionSent target=xuannv（用户主动 intervene 玄女）
        EventKind::UserPrompted { text } if ev.meta.agent == Some(xuannv_id) => {
            store
                .append_message(
                    conv_id,
                    "user",
                    None,
                    "text",
                    &serde_json::json!({ "text": text }),
                    None,
                    Some(&source_id),
                    ev.meta.at,
                )
                .await?;
        }
        EventKind::UserInterventionSent {
            target,
            text,
            attachments,
            system_origin,
            ..
        } if *target == xuannv_id => {
            // 阶段 3：附件 id 列表落 messages.attachments JSON 数组，PWA 历史回显时
            // AttachmentChip 直接拿 id 拼直链 GET /api/uploads/:id 渲缩略。空 = None
            // （让旧前端 fromStoredMessage 走 `Array.isArray` 兜空稳）。
            let attach_json = if attachments.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(
                    attachments
                        .iter()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .collect(),
                ))
            };
            // bug #77：系统注入（bridge / sentinel）的 user_intervention_sent
            // 持久化时 role 写 "system" + content 含 origin，让 fromStoredMessage
            // 历史回放时还原 SystemMessage（玄女侧灰底气泡），不被错误显示成右侧
            // user bubble。普通用户敲键盘的 intervene system_origin=None 仍走 "user"。
            //
            // bug #77 二次修：carbon_copy 跳过——同一次抄送 bridge 已 publish
            // OrchestratorCcReceived（下面有专门 handler 落"system" stored），
            // 这条 user_intervention_sent 是 bridge 把 [CC] 长 prompt 注入玄女
            // cc stdin 的副产物，不能再写库否则历史回放显示两条抄送 bubble。
            if system_origin.as_deref() == Some("carbon_copy") {
                return Ok(());
            }
            let (role, content) = match system_origin.as_deref() {
                Some(origin) => (
                    "system",
                    serde_json::json!({ "text": text, "origin": origin }),
                ),
                None => ("user", serde_json::json!({ "text": text })),
            };
            store
                .append_message(
                    conv_id,
                    role,
                    None,
                    "text",
                    &content,
                    attach_json.as_ref(),
                    Some(&source_id),
                    ev.meta.at,
                )
                .await?;
        }
        // 玄女自己的回应
        EventKind::AgentResponded { text } if ev.meta.agent == Some(xuannv_id) => {
            store
                .append_message(
                    conv_id,
                    "xuannv",
                    Some(&xuannv_id.to_string()),
                    "text",
                    &serde_json::json!({ "text": text }),
                    None,
                    Some(&source_id),
                    ev.meta.at,
                )
                .await?;
        }
        // 玄女派的 task → 卡片消息（前端可点进去看 task chat）
        EventKind::TaskCreated { title, description } if ev.meta.agent == Some(xuannv_id) => {
            let task_id = ev.meta.task.map(|t| t.to_string()).unwrap_or_default();
            store
                .append_message(
                    conv_id,
                    "xuannv",
                    Some(&xuannv_id.to_string()),
                    "task_card",
                    &serde_json::json!({
                        "task_id": task_id,
                        "title": title,
                        "description": description,
                    }),
                    None,
                    Some(&source_id),
                    ev.meta.at,
                )
                .await?;
        }
        // 用户对别的门客说话 → 玄女抄送（呈报）也进玄女主线
        EventKind::OrchestratorCcReceived {
            from_user_to, text, ..
        } if ev.meta.agent == Some(xuannv_id) => {
            store
                .append_message(
                    conv_id,
                    "system",
                    None,
                    "text",
                    &serde_json::json!({
                        "text": text,
                        "cc_from_user_to": from_user_to.to_string(),
                    }),
                    None,
                    Some(&source_id),
                    ev.meta.at,
                )
                .await?;
        }
        _ => {} // 其它事件不入对话视图
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use fuxi_core::{EventMeta, TaskId};
    use std::time::Duration;
    use tempfile::tempdir;

    async fn open_store() -> (tempfile::TempDir, ConvStore) {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");
        (dir, ConvStore::new(pool))
    }

    #[tokio::test]
    async fn ensure_scope_idempotent() {
        let (_dir, store) = open_store().await;
        let id1 = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let id2 = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        assert_eq!(id1, id2, "重复 ensure 应返同 id");
    }

    #[tokio::test]
    async fn append_message_bumps_count_and_last_active() {
        let (_dir, store) = open_store().await;
        let conv = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let now = Utc::now();
        store
            .append_message(
                &conv,
                "user",
                None,
                "text",
                &serde_json::json!({"text":"hi"}),
                None,
                None,
                now,
            )
            .await
            .unwrap();
        store
            .append_message(
                &conv,
                "xuannv",
                None,
                "text",
                &serde_json::json!({"text":"hello"}),
                None,
                None,
                now + chrono::Duration::seconds(1),
            )
            .await
            .unwrap();

        let row =
            sqlx::query("SELECT message_count, last_active_at FROM conversations WHERE id = ?1")
                .bind(&conv)
                .fetch_one(store.handle())
                .await
                .unwrap();
        let count: i64 = row.try_get("message_count").unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn page_messages_returns_ascending_with_pagination() {
        let (_dir, store) = open_store().await;
        let conv = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let base = Utc::now();
        // 灌 5 条
        for i in 0..5 {
            store
                .append_message(
                    &conv,
                    "user",
                    None,
                    "text",
                    &serde_json::json!({"text": format!("m-{i}")}),
                    None,
                    None,
                    base + chrono::Duration::milliseconds(i),
                )
                .await
                .unwrap();
        }

        // limit=3 → 拿最新 3 条（m-2/m-3/m-4 ASC），has_more=true
        let (msgs, has_more, oldest) = store.page_messages(&conv, 3, None).await.unwrap();
        assert_eq!(msgs.len(), 3);
        let labels: Vec<String> = msgs
            .iter()
            .map(|m| m.content["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(labels, vec!["m-2", "m-3", "m-4"]);
        assert!(has_more, "更早的还有 → has_more=true");

        // 拿 oldest 当 cursor 翻历史 → 该看到 m-0/m-1
        let (older, has_more2, _) = store
            .page_messages(&conv, 3, oldest.as_deref())
            .await
            .unwrap();
        let labels2: Vec<String> = older
            .iter()
            .map(|m| m.content["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(labels2, vec!["m-0", "m-1"]);
        assert!(!has_more2, "已到头 → has_more=false");
    }

    #[tokio::test]
    async fn page_messages_empty_returns_empty_vec() {
        let (_dir, store) = open_store().await;
        let conv = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let (msgs, has_more, oldest) = store.page_messages(&conv, 50, None).await.unwrap();
        assert!(msgs.is_empty());
        assert!(!has_more);
        assert!(oldest.is_none());
    }

    #[tokio::test]
    async fn page_messages_unknown_cursor_returns_empty() {
        let (_dir, store) = open_store().await;
        let conv = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let now = Utc::now();
        store
            .append_message(
                &conv,
                "user",
                None,
                "text",
                &serde_json::json!({"text":"a"}),
                None,
                None,
                now,
            )
            .await
            .unwrap();
        let (msgs, has_more, oldest) = store
            .page_messages(&conv, 10, Some("not-a-real-id"))
            .await
            .unwrap();
        assert!(msgs.is_empty());
        assert!(!has_more);
        assert!(oldest.is_none());
    }

    fn user_intervention_event(target: AgentId, text: &str, xuannv_meta_agent: bool) -> Event {
        let mut meta = EventMeta::now();
        if xuannv_meta_agent {
            meta.agent = Some(target);
        }
        Event {
            meta,
            kind: EventKind::UserInterventionSent {
                target,
                mode: "append".into(),
                text: text.into(),
                mentions: vec![target],
                pinned_node: None,
                attachments: Vec::new(),
                system_origin: None,
            },
        }
    }

    fn agent_responded_event(agent: AgentId, text: &str) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        Event {
            meta,
            kind: EventKind::AgentResponded { text: text.into() },
        }
    }

    fn task_created_event(agent: AgentId, task: TaskId, title: &str) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        meta.task = Some(task);
        Event {
            meta,
            kind: EventKind::TaskCreated {
                title: title.into(),
                description: "desc".into(),
            },
        }
    }

    /// EventBus → conv_store sync 的端到端：起 sync task，发玄女事件，等几毫秒，
    /// 然后查 messages 表确认有对应行。
    #[tokio::test]
    async fn sync_translates_xuannv_events_to_messages() {
        use fuxi_events::EventBus;
        let (_dir, store) = open_store().await;
        let store = Arc::new(store);
        let bus = EventBus::with_memory_store().await.unwrap();
        let xuannv = AgentId::new();
        let task = TaskId::new();

        let (_tx, rx) = watch::channel(Some(xuannv));
        let h = spawn_xuannv_sync(store.clone(), bus.clone(), rx).await;

        // user intervene 玄女
        bus.publish(user_intervention_event(xuannv, "你好", false))
            .unwrap();
        // 玄女回应
        bus.publish(agent_responded_event(xuannv, "在的")).unwrap();
        // 玄女派 task
        bus.publish(task_created_event(xuannv, task, "修 ERP-1066"))
            .unwrap();
        // 别人的事件 → 不该入对话
        let other = AgentId::new();
        bus.publish(agent_responded_event(other, "门客噪音"))
            .unwrap();

        // 等 sync flush——broadcast 是 push，所以 yield 几次就到
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let conv = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
            let (msgs, _, _) = store.page_messages(&conv, 50, None).await.unwrap();
            if msgs.len() >= 3 {
                // 三条到位且没噪音
                let kinds: Vec<&str> = msgs.iter().map(|m| m.kind.as_str()).collect();
                assert!(kinds.contains(&"text"));
                assert!(kinds.contains(&"task_card"));
                let texts: Vec<String> = msgs
                    .iter()
                    .filter(|m| m.kind == "text")
                    .filter_map(|m| m.content["text"].as_str().map(String::from))
                    .collect();
                assert!(texts.contains(&"你好".to_string()));
                assert!(texts.contains(&"在的".to_string()));
                assert!(!texts.contains(&"门客噪音".to_string()), "噪音不该入");
                h.abort();
                return;
            }
        }
        panic!("sync 应在 1s 内把 3 条消息写入");
    }

    /// 反回归：handoff 路径切玄女副本（kill 旧 spawn 新）后，新副本 emit 的
    /// 事件要按"新 id"过滤入库。WHY：早期实现把 xuannv_id 按值捕获，handoff
    /// 后 sync 还在拿旧 id 比对 ev.meta.agent，新副本所有事件全部 fall-through
    /// 不写库 → PWA IM 看不到新副本对话。修法：换成 watch::Receiver，每条
    /// 事件取最新 id。
    #[tokio::test]
    async fn sync_picks_up_new_xuannv_id_after_handoff() {
        use fuxi_events::EventBus;
        let (_dir, store) = open_store().await;
        let store = Arc::new(store);
        let bus = EventBus::with_memory_store().await.unwrap();
        let old_xn = AgentId::new();
        let new_xn = AgentId::new();

        let (tx, rx) = watch::channel(Some(old_xn));
        let h = spawn_xuannv_sync(store.clone(), bus.clone(), rx).await;

        // 旧副本说一句
        bus.publish(agent_responded_event(old_xn, "旧副本：我下班了"))
            .unwrap();

        // 等旧副本那条落库（避免后面查"新副本是否落库"被误识为旧副本未落）
        let conv = store.ensure_scope(SCOPE_XUANNV, None).await.unwrap();
        let mut old_landed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let (msgs, _, _) = store.page_messages(&conv, 50, None).await.unwrap();
            if msgs
                .iter()
                .any(|m| m.content["text"].as_str() == Some("旧副本：我下班了"))
            {
                old_landed = true;
                break;
            }
        }
        assert!(old_landed, "旧副本发言应先落库");

        // handoff：watch 切到新副本 id
        tx.send(Some(new_xn)).expect("watch send 新 id 不该失败");

        // 新副本继续发言——这条在 buggy 版本不会落库
        bus.publish(agent_responded_event(new_xn, "新副本：接班完成"))
            .unwrap();

        // 等新副本那条落库
        let mut new_landed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let (msgs, _, _) = store.page_messages(&conv, 50, None).await.unwrap();
            if msgs
                .iter()
                .any(|m| m.content["text"].as_str() == Some("新副本：接班完成"))
            {
                new_landed = true;
                break;
            }
        }
        assert!(new_landed, "handoff 后新副本发言必须按新 id 过滤通过并落库");

        h.abort();
    }
}
