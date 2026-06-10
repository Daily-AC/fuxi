//! `TopicXuannvSpawner`——把 orchestrator 的 [`XuannvSpawner`] 接到 fuxi-cli 的真
//! spawn 链路（块5 / task#7.1）。
//!
//! 反向依赖 pattern（同 recall_sink / pending_sink）：trait 在 orchestrator，复用
//! spawn_with_prelude + conv_store 的具体实现放 fuxi-cli。
//!
//! 行为：`ensure_xuannv_for_topic` 池 miss 时调本 adapter——拉该 topic 的对话历史拼
//! prelude（[`crate::topic_switch::build_topic_prelude`]，自带「你被切到 topic X」导语
//! = 服务该 topic 的 addendum）→ [`crate::xuannv_handoff::spawn_with_prelude`] 起新 cc
//! 分身（session_id None 红线已守）→ `set_xuannv_for_topic` 入池 → 返回新 id。
//!
//! WHY 持 `Weak<Fuxi>`：Fuxi 持有本 spawner（Arc<dyn>），本 spawner 又要回调
//! `fuxi.set_xuannv_for_topic`——持 Arc 会成引用环泄漏。Weak + upgrade，Fuxi 已 drop
//! 时 spawn 静默失败（进程在关停，无所谓）。

use async_trait::async_trait;
use fuxi_core::TopicId;
use fuxi_core::id::AgentId;
use fuxi_im::conv_store::{ConvStore, SCOPE_XUANNV};
use fuxi_im::pending_notify::PendingNotifyStore;
use fuxi_im::topic_store::TopicStore;
use fuxi_memory::OracleStore;
use fuxi_orchestrator::{Fuxi, OrchestratorError, Result as OrchResult, XuannvSpawner};
use std::sync::Weak;

/// 拉某 topic 历史拼 prelude 时的最近消息条数——同 topic_switch 口径。
const RECENT_MESSAGES: usize = crate::topic_switch::DEFAULT_RECENT_MESSAGES;

pub struct TopicXuannvSpawner {
    fuxi: Weak<Fuxi>,
    oracle: OracleStore,
    role: String,
    conv_store: ConvStore,
    topic_store: TopicStore,
    pending_store: PendingNotifyStore,
}

impl TopicXuannvSpawner {
    pub fn new(
        fuxi: Weak<Fuxi>,
        oracle: OracleStore,
        role: String,
        conv_store: ConvStore,
        topic_store: TopicStore,
        pending_store: PendingNotifyStore,
    ) -> Self {
        Self {
            fuxi,
            oracle,
            role,
            conv_store,
            topic_store,
            pending_store,
        }
    }

    /// 块5：分身刚 spawn 入池后，drain 该 topic 的持久队列把 dormant 期间攒下的
    /// 完工/里程碑信号补发给它（intervene_system 注入首 turn）→ mark_delivered
    /// 幂等不重投。失败只 warn 不挡 spawn——分身已活，补发是 best-effort（drain
    /// 没 mark 的下次 respawn 还会再 drain，不丢）。
    async fn drain_pending_to(&self, fuxi: &Fuxi, topic: TopicId, agent: AgentId) {
        let pending = match self
            .pending_store
            .drain_undelivered(&topic.as_uuid().to_string())
            .await
        {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(%topic, error = %e, "drain_pending: 读持久队列失败，跳过补发");
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        tracing::info!(%topic, xuannv = %agent, n = pending.len(), "drain_pending: 补发 dormant 期间攒下的信号");
        for p in pending {
            // 落库的就是活路径同源 prompt（块4 enqueue_dormant_milestone），直接注入。
            if let Err(e) = fuxi
                .intervene_system_origin(agent, true, &p.prompt, p.system_origin.clone())
                .await
            {
                // 注入失败：**不** mark_delivered，留队列等下次 respawn 重投（信号不丢）。
                tracing::warn!(%topic, id = %p.id, error = %e, "drain_pending: 补发注入失败，留队列重投");
                continue;
            }
            if let Err(e) = self.pending_store.mark_delivered(&p.id).await {
                tracing::warn!(%topic, id = %p.id, error = %e, "drain_pending: mark_delivered 失败（可能重投一次，幂等可容忍）");
            }
        }
    }

    /// 拉该 topic 的对话历史（topic 过滤，不混别的 topic）。失败返空——空回顾仍能
    /// spawn，玄女首条消息靠 events.db 兜底，不让历史 IO 失败挡死懒启动。
    async fn load_topic_history(&self, topic: TopicId) -> Vec<fuxi_im::conv_store::Message> {
        let conv_id = match self.conv_store.ensure_scope(SCOPE_XUANNV, None).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(%topic, error = %e, "spawn_for_topic: ensure_scope 失败，prelude 走空回顾");
                return Vec::new();
            }
        };
        match self
            .conv_store
            .page_messages_in_topic(
                &conv_id,
                &topic.as_uuid().to_string(),
                RECENT_MESSAGES,
                None,
            )
            .await
        {
            Ok((msgs, _has_more, _oldest)) => msgs,
            Err(e) => {
                tracing::warn!(%topic, error = %e, "spawn_for_topic: 拉 topic 历史失败，prelude 走空回顾");
                Vec::new()
            }
        }
    }
}

#[async_trait]
impl XuannvSpawner for TopicXuannvSpawner {
    async fn spawn_for_topic(&self, topic: TopicId) -> OrchResult<AgentId> {
        let fuxi = self
            .fuxi
            .upgrade()
            .ok_or_else(|| OrchestratorError::Other("Fuxi 已 drop，跳过 spawn_for_topic".into()))?;

        // topic 标题给 prelude 导语用；查不到/不存在用 fallback（不阻断 spawn）。
        let title = match self.topic_store.get(topic).await {
            Ok(Some(meta)) => meta.title,
            _ => topic.to_string(),
        };
        let recent = self.load_topic_history(topic).await;
        let prelude = crate::topic_switch::build_topic_prelude(&title, &recent);

        // spawn_with_prelude 内部 session_id None / resume None（红线，已守）。
        // 块5：注入 FUXI_TOPIC，让分身 shell 的 fuxi dispatch 默认带 --topic（worker
        // 事件归位本 topic）。
        let id = crate::xuannv_handoff::spawn_with_prelude(
            &fuxi,
            &self.oracle,
            &self.role,
            &prelude,
            vec![("FUXI_TOPIC".to_string(), topic.as_uuid().to_string())],
        )
        .await
        .map_err(|e| OrchestratorError::Other(format!("spawn_for_topic 起新分身失败: {e}")))?;

        // 入池——绑 topic → 新分身（general topic 还会同步 xuannv_id watch 镜像）。
        fuxi.set_xuannv_for_topic(topic, id).await;
        tracing::info!(%topic, xuannv = %id, role = %self.role, "spawn_for_topic: 玄女分身已懒启动入池");

        // 块5：drain 该 topic 的持久队列补发 dormant 期间攒下的信号（a01cfab5 收口）。
        // 必须在入池后——补发走 intervene 要分身已在 shelf。
        self.drain_pending_to(&fuxi, topic, id).await;
        Ok(id)
    }
}
