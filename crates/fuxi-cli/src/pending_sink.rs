//! `PendingNotifyStoreSink`——把 orchestrator 的 [`PendingNotifySink`] 接到 fuxi-im 的
//! [`PendingNotifyStore`]（块5 / a01cfab5「信号不丢」补发链路）。
//!
//! 反向依赖 pattern（同 `recall_sink.rs` / `extractor_hook.rs`）：trait 定义在
//! fuxi-orchestrator，写 im.db 的 impl 放 fuxi-cli。fuxi-orchestrator 不能依赖
//! fuxi-im（循环）；fuxi-cli 顶层依赖一切，是注入 adapter 的合适位置。
//!
//! 边界职责：orchestrator 侧用 typed [`TopicId`]；store 侧的 `enqueue` 收 `&str`。
//! stringify 收敛在本 adapter 这唯一一处（[`TopicId::as_uuid`] → string），上游
//! 全程 typed，下游持久层裸 string——边界清晰。

use async_trait::async_trait;
use fuxi_core::TopicId;
use fuxi_im::pending_notify::PendingNotifyStore;
use fuxi_orchestrator::{PendingNotifySink, Result as OrchResult};

/// 把 `PendingNotifySink` 接到 `PendingNotifyStore`。clone 廉价（store 内部 Arc pool）。
pub struct PendingNotifyStoreSink {
    store: PendingNotifyStore,
}

impl PendingNotifyStoreSink {
    pub fn new(store: PendingNotifyStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl PendingNotifySink for PendingNotifyStoreSink {
    async fn enqueue(
        &self,
        topic_id: TopicId,
        prompt: &str,
        system_origin: &str,
    ) -> OrchResult<()> {
        // store.enqueue 返回生成的 id（拿来关联日志），sink 契约只关心成败 → 丢 id。
        // 失败映射到 orchestrator 的错误类型，让 bridge 侧 warn「信号可能丢失」可见。
        self.store
            .enqueue(&topic_id.as_uuid().to_string(), prompt, system_origin)
            .await
            .map(|_id| ())
            .map_err(|e| {
                fuxi_orchestrator::OrchestratorError::Other(format!("pending enqueue: {e}"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// adapter 把 typed TopicId stringify 后真落进 store，drain 拿得回——验证
    /// 边界 stringify（TopicId → uuid string）跟 store 的 topic_id 列对齐。
    #[tokio::test]
    async fn sink_enqueues_into_store_and_drains_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = fuxi_im::db::init_at(dir.path().join("im.db"))
            .await
            .expect("init im.db");
        let store = PendingNotifyStore::new(pool);
        let sink = PendingNotifyStoreSink::new(store.clone());

        let topic = TopicId::new();
        sink.enqueue(topic, "[REVIEW_REQUEST] 门客求审", "review_request")
            .await
            .expect("enqueue 应成功");

        // 用 store 直接 drain 验证落库的 topic_id 跟 adapter stringify 出来的一致。
        let pending = store
            .drain_undelivered(&topic.as_uuid().to_string())
            .await
            .expect("drain");
        assert_eq!(pending.len(), 1, "应取回刚入队的一条");
        assert_eq!(pending[0].topic_id, topic.as_uuid().to_string());
        assert_eq!(pending[0].system_origin, "review_request");
        assert!(pending[0].prompt.contains("[REVIEW_REQUEST]"));
    }
}
