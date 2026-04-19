//! 跨 crate 读接口：按 id 取 trigger 的自然语言 `intent`。
//!
//! 为什么放 core：fuxi-orchestrator 里的 SystemEventBridge 见到 `TriggerFired`
//! 需要拼三段式 prompt，而 intent 只在 fuxi-scheduler 的 `TriggerStore` 里。
//! 直接互依 → 循环。抽到 core 让双方都依赖同一个抽象。
//!
//! 实装侧（fuxi-scheduler）只需 `impl TriggerLookup for TriggerStore`。

use async_trait::async_trait;

/// 只读接口：拿到 trigger 的 `intent`。未找到返回 `None`。
#[async_trait]
pub trait TriggerLookup: Send + Sync {
    async fn intent(&self, id: &str) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    struct FakeLookup {
        map: HashMap<String, String>,
    }

    #[async_trait]
    impl TriggerLookup for FakeLookup {
        async fn intent(&self, id: &str) -> Option<String> {
            self.map.get(id).cloned()
        }
    }

    #[tokio::test]
    async fn returns_intent_for_known_id() {
        let mut map = HashMap::new();
        map.insert("trg_1".to_string(), "每天早 9 点 review".to_string());
        let lookup: Arc<dyn TriggerLookup> = Arc::new(FakeLookup { map });
        assert_eq!(
            lookup.intent("trg_1").await.as_deref(),
            Some("每天早 9 点 review")
        );
    }

    #[tokio::test]
    async fn returns_none_for_unknown_id() {
        let lookup: Arc<dyn TriggerLookup> = Arc::new(FakeLookup {
            map: HashMap::new(),
        });
        assert!(lookup.intent("nope").await.is_none());
    }
}
