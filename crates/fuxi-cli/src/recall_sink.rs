//! `OracleRecallSink`——把 dispatch pump 的 task 完成 context 接到策府。
//!
//! 反向依赖 pattern（同 `extractor_hook.rs`）：trait 定义在 fuxi-orchestrator，
//! 具体写 OracleStore 的 impl 放在 fuxi-cli。fuxi-orchestrator 不能依赖 fuxi-memory
//! （循环）；fuxi-cli 顶层依赖一切，是注入这种 adapter 的合适位置。
//!
//! ## 写多条 fact 而不是一条
//!
//! 每次 task Done 写最多 4 条（视 context 字段 None / Some 而定）：
//!
//! | subject       | predicate     | object         | 何时写 |
//! |---------------|---------------|----------------|----|
//! | `task-<id>`   | `worktree`    | `<path>`       | worktree.is_some() |
//! | `task-<id>`   | `session_id`  | `<sid>`        | cli_session_id.is_some() |
//! | `role-<role>` | `worktree`    | `<path>`       | worktree.is_some() |
//! | `role-<role>` | `session_id`  | `<sid>`        | cli_session_id.is_some() |
//!
//! 召回查询：`--recall-task <id>` 拿 `task-<id>` 的两个 predicate；`--recall-role`
//! 拿 `role-<role>` 的两个（query_one 取 updated_at DESC 最新）。session_id 缺失
//! 走"无 session 的复用 worktree"路径（codex 永远走这条；cc 偶发也成立）。
//!
//! ## 同 session 跨多 task 是设计
//!
//! cc `--resume` 是 session 粒度——一次 session 跨多 task 共享 history。append-only
//! 不去重符合 oracle 既有语义，updated_at DESC 自然取最新。

use async_trait::async_trait;
use fuxi_memory::{NewFact, OracleStore};
use fuxi_orchestrator::{RecallContext, RecallSink};

/// 召回入库 source 标记——和手动 `fuxi memory record` 区分，便于 audit。
const RECALL_SOURCE: &str = "auto:dispatch-pump";

const SESSION_PREDICATE: &str = "session_id";
const WORKTREE_PREDICATE: &str = "worktree";

/// 把 `RecallSink` 接到 `OracleStore`。
///
/// 不再持 `Arc<Fuxi>` 反查 role——L2 改成 pump 在 RecallContext 里直接送 role
/// 和 worktree 进来，sink 纯写不查。这样 sink 完全不依赖 orchestrator runtime
/// 状态，能在测试里裸跑。
pub struct OracleRecallSink {
    oracle: OracleStore,
}

impl OracleRecallSink {
    pub fn new(oracle: OracleStore) -> Self {
        Self { oracle }
    }

    /// 写一条 fact，失败只 warn 不传播——召回入库是 best-effort。
    async fn try_write(&self, subject: String, predicate: &str, object: &str, ctx_label: &str) {
        if let Err(e) = self
            .oracle
            .insert(NewFact::new(subject.clone(), predicate, object).with_source(RECALL_SOURCE))
            .await
        {
            tracing::warn!(
                ctx = %ctx_label,
                %subject,
                predicate,
                error = %e,
                "recall: 入库失败（best-effort 不传播）"
            );
        }
    }
}

#[async_trait]
impl RecallSink for OracleRecallSink {
    async fn record(&self, ctx: RecallContext) {
        let task_subject = format!("task-{}", task_id_uuid(&ctx.task_id));
        let role_subject = format!("role-{}", ctx.role);
        let label = format!(
            "agent={} task={} role={}",
            ctx.agent_id, ctx.task_id, ctx.role
        );

        if let Some(wt) = ctx.worktree.as_ref() {
            let path_str = wt.to_string_lossy().into_owned();
            self.try_write(task_subject.clone(), WORKTREE_PREDICATE, &path_str, &label)
                .await;
            self.try_write(role_subject.clone(), WORKTREE_PREDICATE, &path_str, &label)
                .await;
        }
        if let Some(sid) = ctx.cli_session_id.as_ref() {
            self.try_write(task_subject, SESSION_PREDICATE, sid, &label)
                .await;
            self.try_write(role_subject, SESSION_PREDICATE, sid, &label)
                .await;
        }
        if ctx.worktree.is_none() && ctx.cli_session_id.is_none() {
            tracing::debug!(
                ctx = %label,
                "recall: 无 worktree 也无 session_id（玄女这类）—— sink 不写 fact"
            );
        }
    }
}

/// `TaskId` 的 Display 是 `task-<uuid>`；提取裸 uuid 部分避免拼成 `task-task-<uuid>`。
fn task_id_uuid(task_id: &fuxi_core::id::TaskId) -> String {
    let s = task_id.to_string();
    s.strip_prefix("task-").unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::id::{AgentId, TaskId};
    use std::path::PathBuf;

    fn ctx_with(role: &str, worktree: Option<&str>, sid: Option<&str>) -> RecallContext {
        RecallContext {
            agent_id: AgentId::new(),
            task_id: TaskId::new(),
            role: role.to_string(),
            worktree: worktree.map(PathBuf::from),
            cli_session_id: sid.map(String::from),
        }
    }

    /// cc 完整 context：worktree + session_id 都有 → 写 4 条 fact。
    #[tokio::test]
    async fn record_writes_four_facts_when_full_context() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let sink = OracleRecallSink::new(oracle.clone());

        let ctx = ctx_with("luban", Some("/tmp/wt-1"), Some("sess-abc"));
        let task_subject = format!("task-{}", task_id_uuid(&ctx.task_id));
        sink.record(ctx).await;

        // task-* 两条
        let t_wt = oracle
            .query_one(&task_subject, WORKTREE_PREDICATE)
            .await
            .unwrap()
            .expect("task worktree fact");
        assert_eq!(t_wt.object, "/tmp/wt-1");
        let t_sid = oracle
            .query_one(&task_subject, SESSION_PREDICATE)
            .await
            .unwrap()
            .expect("task session fact");
        assert_eq!(t_sid.object, "sess-abc");
        // role-* 两条
        assert_eq!(
            oracle
                .query_one("role-luban", WORKTREE_PREDICATE)
                .await
                .unwrap()
                .unwrap()
                .object,
            "/tmp/wt-1"
        );
        assert_eq!(
            oracle
                .query_one("role-luban", SESSION_PREDICATE)
                .await
                .unwrap()
                .unwrap()
                .object,
            "sess-abc"
        );
    }

    /// codex 路径：worktree 有但 cli_session_id=None → 只写 worktree 两条。
    /// 这是 L2 关键场景：让 codex 也能进 sink，召回时复用 worktree 看上次的成果。
    #[tokio::test]
    async fn codex_record_writes_only_worktree_facts() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let sink = OracleRecallSink::new(oracle.clone());

        let ctx = ctx_with("luban-codex", Some("/tmp/wt-codex"), None);
        let task_subject = format!("task-{}", task_id_uuid(&ctx.task_id));
        sink.record(ctx).await;

        assert!(
            oracle
                .query_one(&task_subject, WORKTREE_PREDICATE)
                .await
                .unwrap()
                .is_some(),
            "task worktree 应入库"
        );
        assert!(
            oracle
                .query_one(&task_subject, SESSION_PREDICATE)
                .await
                .unwrap()
                .is_none(),
            "无 session 时 task session fact 不应入库"
        );
        assert!(
            oracle
                .query_one("role-luban-codex", WORKTREE_PREDICATE)
                .await
                .unwrap()
                .is_some(),
            "role worktree 应入库"
        );
        assert!(
            oracle
                .query_one("role-luban-codex", SESSION_PREDICATE)
                .await
                .unwrap()
                .is_none(),
            "无 session 时 role session fact 不应入库"
        );
    }

    /// 同 role 多次 record → query_one 取最新 updated_at（β 的 --recall-role 路径）。
    #[tokio::test]
    async fn recall_role_query_returns_latest_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let sink = OracleRecallSink::new(oracle.clone());

        sink.record(ctx_with("luban", Some("/tmp/wt-old"), Some("sess-old")))
            .await;
        // 跨秒确保 updated_at 不撞——sqlite ts 精度到秒级。
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        sink.record(ctx_with("luban", Some("/tmp/wt-new"), Some("sess-new")))
            .await;

        assert_eq!(
            oracle
                .query_one("role-luban", SESSION_PREDICATE)
                .await
                .unwrap()
                .unwrap()
                .object,
            "sess-new",
            "session 应取最新"
        );
        assert_eq!(
            oracle
                .query_one("role-luban", WORKTREE_PREDICATE)
                .await
                .unwrap()
                .unwrap()
                .object,
            "/tmp/wt-new",
            "worktree 应取最新"
        );
    }

    /// 玄女这种无 worktree 也无 session 的 ctx → sink noop（不 panic、不写脏 fact）。
    #[tokio::test]
    async fn record_with_neither_worktree_nor_session_writes_nothing() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let sink = OracleRecallSink::new(oracle.clone());

        let ctx = ctx_with("xuannv", None, None);
        let task_subject = format!("task-{}", task_id_uuid(&ctx.task_id));
        sink.record(ctx).await;

        assert!(
            oracle.query(&task_subject, 10).await.unwrap().is_empty(),
            "task subject 不应有任何 fact"
        );
        assert!(
            oracle.query("role-xuannv", 10).await.unwrap().is_empty(),
            "role subject 不应有任何 fact"
        );
    }
}
