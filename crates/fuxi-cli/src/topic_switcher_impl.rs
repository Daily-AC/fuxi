//! Phase 1 · `XuannvSwitcher` 在 fuxi-cli 的实现——把 PWA HTTP 入口与
//! `topic_switch::switch_topic_to` 焊起来。
//!
//! 反向依赖（[`fuxi_orchestrator::XuannvSwitcher`] 文档）：trait 在 orchestrator
//! 让 fuxi-im handler 调；具体 spawn cc 走 fuxi-cli 才能装齐 fuxi-skills /
//! fuxi-agent-cc。本 file 把 fuxi + topic_store 闭包进来，HTTP handler 只需传
//! topic_id。
//!
//! Phase 2：切换不再 spawn，oracle/conv_store/role 移交 TopicXuannvSpawner
//! （`ensure_xuannv_for_topic` 懒启动时才用），switcher 只留验在 + ensure 路由。
//!
//! 启动期 fuxi-cli `im.rs` 构造一份注入 `Fuxi::set_xuannv_switcher`。

use async_trait::async_trait;
use fuxi_core::TopicId;
use fuxi_im::topic_store::TopicStore;
use fuxi_orchestrator::{Fuxi, OrchestratorError, Result, XuannvSwitcher};
use std::sync::Arc;

/// 把 [`crate::topic_switch::switch_topic_to`] 包成 trait object 供 fuxi-im
/// handler 调。handler 只传 topic_id。
pub struct CliXuannvSwitcher {
    fuxi: Arc<Fuxi>,
    topic_store: TopicStore,
}

impl CliXuannvSwitcher {
    pub fn new(fuxi: Arc<Fuxi>, topic_store: TopicStore) -> Self {
        Self { fuxi, topic_store }
    }
}

#[async_trait]
impl XuannvSwitcher for CliXuannvSwitcher {
    async fn switch_topic(&self, topic_id: TopicId) -> Result<()> {
        crate::topic_switch::switch_topic_to(self.fuxi.as_ref(), &self.topic_store, topic_id)
            .await
            .map_err(|e| OrchestratorError::Other(format!("switch_topic_to: {e}")))
    }
}
