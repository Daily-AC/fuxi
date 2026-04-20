//! `OracleRecallSink`——把 dispatch pump 的 task 完成钩子接到策府。
//!
//! 反向依赖 pattern（同 `extractor_hook.rs`）：trait 定义在 fuxi-orchestrator，
//! 具体写 OracleStore 的 impl 放在 fuxi-cli。fuxi-orchestrator 不能依赖 fuxi-memory
//! （循环）；fuxi-cli 顶层依赖一切，是注入这种 adapter 的合适位置。
//!
//! ## 写两条 fact 而不是一条
//!
//! `--recall-task <id>` 走 `subject = task-<id>` 单值召回；
//! `--recall-role <role>` 走 `subject = role-<role>` 取 `query_one`（updated_at DESC）
//! 拿到该 role 最近一次完成 task 的 session_id。两条 fact 由本 sink 一次写入：
//! 同事务两条 insert，共用 source `auto:dispatch-pump`，方便后续审计。
//!
//! ## 多个 task 指向同 session 是设计
//!
//! cc `--resume` 是 session 粒度——一次 session 跨多个 task 共享 history。
//! 所以同 session 下做 A / B 两 task 会写两条 `task-A` / `task-B` 都指 sess-X，
//! `role-luban` 只保留最新（updated_at DESC）。append-only 不去重符合 oracle 既有语义。

use async_trait::async_trait;
use fuxi_core::id::{AgentId, TaskId};
use fuxi_memory::{NewFact, OracleStore};
use fuxi_orchestrator::{Fuxi, RecallSink};
use std::sync::Arc;

/// 召回入库 source 标记——和手动 `fuxi memory record` 区分，便于 audit。
const RECALL_SOURCE: &str = "auto:dispatch-pump";

/// session_id 在策府里的统一 predicate——和 `session.rs::SESSION_PREDICATE` 一致。
const SESSION_PREDICATE: &str = "session_id";

/// 把 `RecallSink` 接到 `OracleStore`。
///
/// 持有 `Arc<Fuxi>` 才能 `list_workers()` 反查 agent → role；持有 `OracleStore`
/// 才能写两条 fact。两个 handle 都 cheap clone（pool / Arc 语义）。
pub struct OracleRecallSink {
    oracle: OracleStore,
    fuxi: Arc<Fuxi>,
}

impl OracleRecallSink {
    pub fn new(oracle: OracleStore, fuxi: Arc<Fuxi>) -> Self {
        Self { oracle, fuxi }
    }

    /// 反查 agent_id → role。失败（agent 已被 kill）返 None——caller 此时只写 task 不写 role。
    async fn role_of(&self, agent_id: AgentId) -> Option<String> {
        self.fuxi
            .list_workers()
            .await
            .into_iter()
            .find(|c| c.id == agent_id)
            .map(|c| c.profile.role)
    }
}

#[async_trait]
impl RecallSink for OracleRecallSink {
    async fn record_task_session(&self, agent_id: AgentId, task_id: TaskId, session_id: String) {
        // task-<id> 入库——recall-task 走它。
        let task_subject = format!("task-{}", task_id_uuid(task_id));
        if let Err(e) = self
            .oracle
            .insert(
                NewFact::new(task_subject, SESSION_PREDICATE, &session_id)
                    .with_source(RECALL_SOURCE),
            )
            .await
        {
            tracing::warn!(
                %agent_id, %task_id, error = %e,
                "recall: 入库 task→session 失败（best-effort 不传播）"
            );
        }

        // role-<role> 入库——recall-role 取 query_one 拿最新（updated_at DESC）。
        // role 反查不到（agent 已 dead）就只留 task fact——不影响主路径。
        if let Some(role) = self.role_of(agent_id).await {
            let role_subject = format!("role-{role}");
            if let Err(e) = self
                .oracle
                .insert(
                    NewFact::new(role_subject, SESSION_PREDICATE, &session_id)
                        .with_source(RECALL_SOURCE),
                )
                .await
            {
                tracing::warn!(
                    %agent_id, %task_id, error = %e,
                    "recall: 入库 role→session 失败（best-effort）"
                );
            }
        } else {
            tracing::debug!(
                %agent_id, %task_id,
                "recall: agent 不在 shelf（已 kill？）—— 跳 role 入库，只写 task"
            );
        }
    }
}

/// `TaskId` 的 Display 是 `task-<uuid>`；提取裸 uuid 部分。
/// why 提取：避免拼成 `task-task-<uuid>`——subject 被双前缀污染。
fn task_id_uuid(task_id: TaskId) -> String {
    let s = task_id.to_string();
    s.strip_prefix("task-").unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::agent::{AgentCard, AgentProfile, AgentStatus};
    use fuxi_events::EventBus;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;

    async fn make_fuxi() -> Arc<Fuxi> {
        use fuxi_orchestrator::FuxiConfig;
        let bus = EventBus::with_memory_store().await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            tmp.path().to_path_buf(),
        ));
        let cfg = FuxiConfig {
            allocate_worktree: false,
            ..Default::default()
        };
        Arc::new(Fuxi::with_config(bus, ws, cfg))
    }

    /// 必须的最小 agent stub——insert_agent 要 Arc<dyn Agent>。
    /// 只为测 `role_of` 反查；其他方法走默认 trait 实现就够。
    struct NopAgent {
        card: AgentCard,
    }

    #[async_trait]
    impl fuxi_core::agent::Agent for NopAgent {
        fn card(&self) -> &AgentCard {
            &self.card
        }
        async fn dispatch(
            &self,
            _task: fuxi_core::task::Task,
        ) -> fuxi_core::Result<tokio::sync::mpsc::Receiver<fuxi_core::Event>> {
            let (_, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
        async fn send_message(&self, _t: TaskId, _text: &str) -> fuxi_core::Result<()> {
            Ok(())
        }
        async fn cancel(&self, _t: TaskId) -> fuxi_core::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> fuxi_core::Result<()> {
            Ok(())
        }
    }

    fn make_nop_agent(role: &str) -> Arc<NopAgent> {
        Arc::new(NopAgent {
            card: AgentCard {
                id: AgentId::new(),
                profile: AgentProfile {
                    name: format!("nop-{role}"),
                    role: role.to_string(),
                    cli: "stub".into(),
                    system_prompt: String::new(),
                    tags: vec![],
                    extra: Default::default(),
                },
                endpoint: "stub://local".into(),
                status: AgentStatus::Idle,
            },
        })
    }

    /// 调用 record 后 oracle 应有 `task-<uuid>` 和 `role-<role>` 两条 fact，
    /// 都指向同一 session_id。
    #[tokio::test]
    async fn record_writes_both_task_and_role_facts() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let fuxi = make_fuxi().await;

        let agent = make_nop_agent("luban");
        let agent_id = agent.card.id;
        fuxi.insert_agent(agent.clone(), None).await;

        let task_id = TaskId::new();
        let sink = OracleRecallSink::new(oracle.clone(), fuxi.clone());
        sink.record_task_session(agent_id, task_id, "sess-abc-123".into())
            .await;

        // task-<id> 命中
        let task_subject = format!("task-{}", task_id_uuid(task_id));
        let t = oracle
            .query_one(&task_subject, SESSION_PREDICATE)
            .await
            .unwrap()
            .expect("task fact 应入库");
        assert_eq!(t.object, "sess-abc-123");
        assert_eq!(t.source, RECALL_SOURCE);

        // role-luban 命中
        let r = oracle
            .query_one("role-luban", SESSION_PREDICATE)
            .await
            .unwrap()
            .expect("role fact 应入库");
        assert_eq!(r.object, "sess-abc-123");
    }

    /// agent 已被 kill（不在 shelf）时只写 task 不写 role——best-effort 不 panic。
    #[tokio::test]
    async fn record_when_agent_unknown_writes_only_task_fact() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let fuxi = make_fuxi().await;

        let stranger = AgentId::new(); // 未 insert 到 shelf
        let task_id = TaskId::new();
        let sink = OracleRecallSink::new(oracle.clone(), fuxi.clone());
        sink.record_task_session(stranger, task_id, "sess-xyz".into())
            .await;

        let task_subject = format!("task-{}", task_id_uuid(task_id));
        assert!(
            oracle
                .query_one(&task_subject, SESSION_PREDICATE)
                .await
                .unwrap()
                .is_some(),
            "task fact 仍应入库"
        );

        // 任何 role 都不该有
        let role_facts = oracle.query("role-stranger", 10).await.unwrap();
        assert!(role_facts.is_empty(), "未知 agent 不应写 role fact");
    }

    /// 同 role 多次 record → query_one 取最新 updated_at（β 的 --recall-role 路径）。
    #[tokio::test]
    async fn recall_role_query_returns_latest_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let fuxi = make_fuxi().await;

        let agent = make_nop_agent("luban");
        let agent_id = agent.card.id;
        fuxi.insert_agent(agent.clone(), None).await;

        let sink = OracleRecallSink::new(oracle.clone(), fuxi.clone());

        // 第一次 task → sess-old
        sink.record_task_session(agent_id, TaskId::new(), "sess-old".into())
            .await;
        // 让时钟前进确保 updated_at 不撞——sqlite ts 精度到秒级；用 sleep 1s 跨秒。
        // why 不 mock 时钟：oracle insert 直接用 Utc::now()——单测在不动 store 的前提下
        // 只能靠真实时间间隔。1s 在 CI 也接受。
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        sink.record_task_session(agent_id, TaskId::new(), "sess-new".into())
            .await;

        let r = oracle
            .query_one("role-luban", SESSION_PREDICATE)
            .await
            .unwrap()
            .expect("role fact 应有");
        assert_eq!(r.object, "sess-new", "应取最新 session");
    }
}
