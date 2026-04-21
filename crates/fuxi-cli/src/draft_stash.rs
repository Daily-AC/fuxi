//! Draft stash · 切 agent 时保存未发送草稿。
//!
//! 用户在玄女面板打了半句话，点一个门客切过去——回来时那半句要在。
//! stash 按 `ActiveTarget` 分桶；切走时 `stash(target, text)`，切回时
//! `pop(target)` 取出。
//!
//! WHY HashMap 而非 Vec + 找：O(1) 取；ActiveTarget 实现 Hash+Eq（见 repl.rs）。
use std::collections::HashMap;

use crate::repl::ActiveTarget;

#[derive(Debug, Default)]
pub struct DraftStash {
    drafts: HashMap<ActiveTarget, String>,
}

impl DraftStash {
    pub fn new() -> Self {
        Self::default()
    }

    // 宿主未主动用 len/is_empty——测试/未来 UI "待恢复草稿数" 徽章会用。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.drafts.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }

    /// 存草稿。空串视同 `take`——清掉那个 target 的存槽。
    ///
    /// WHY 空串 = 清：用户把草稿手动删光再切走，回来不该弹残留。
    pub fn stash(&mut self, target: ActiveTarget, text: impl Into<String>) {
        let text = text.into();
        if text.is_empty() {
            self.drafts.remove(&target);
        } else {
            self.drafts.insert(target, text);
        }
    }

    /// 取出草稿（`remove`——消费语义）。不存在返回 None。
    pub fn pop(&mut self, target: ActiveTarget) -> Option<String> {
        self.drafts.remove(&target)
    }

    /// 不消费地窥看——未来 UI 在面板标题显示"(有草稿)" 徽章会用。
    #[allow(dead_code)]
    pub fn peek(&self, target: ActiveTarget) -> Option<&str> {
        self.drafts.get(&target).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::AgentId;

    fn worker() -> ActiveTarget {
        ActiveTarget::Worker(AgentId::new())
    }

    #[test]
    fn stash_preserves_per_target() {
        let mut s = DraftStash::new();
        let w1 = worker();
        let w2 = worker();

        s.stash(ActiveTarget::Xuannv, "to xuannv");
        s.stash(w1, "to w1");
        s.stash(w2, "to w2");

        assert_eq!(s.len(), 3);
        assert_eq!(s.pop(w1).as_deref(), Some("to w1"));
        assert_eq!(s.pop(ActiveTarget::Xuannv).as_deref(), Some("to xuannv"));
        assert_eq!(s.pop(w2).as_deref(), Some("to w2"));
    }

    #[test]
    fn pop_is_consuming() {
        let mut s = DraftStash::new();
        s.stash(ActiveTarget::Xuannv, "once");
        assert_eq!(s.pop(ActiveTarget::Xuannv).as_deref(), Some("once"));
        assert_eq!(s.pop(ActiveTarget::Xuannv), None, "pop 两次第二次应 None");
    }

    #[test]
    fn stash_empty_clears() {
        let mut s = DraftStash::new();
        s.stash(ActiveTarget::Xuannv, "content");
        s.stash(ActiveTarget::Xuannv, "");
        assert_eq!(s.pop(ActiveTarget::Xuannv), None, "空串应清掉存槽");
    }

    #[test]
    fn stash_overwrites_same_target() {
        let mut s = DraftStash::new();
        s.stash(ActiveTarget::Xuannv, "first");
        s.stash(ActiveTarget::Xuannv, "second");
        assert_eq!(s.pop(ActiveTarget::Xuannv).as_deref(), Some("second"));
    }

    #[test]
    fn peek_does_not_consume() {
        let mut s = DraftStash::new();
        s.stash(ActiveTarget::Xuannv, "alive");
        assert_eq!(s.peek(ActiveTarget::Xuannv), Some("alive"));
        assert_eq!(s.pop(ActiveTarget::Xuannv).as_deref(), Some("alive"));
    }

    #[test]
    fn empty_stash_pop_returns_none() {
        let mut s = DraftStash::new();
        assert_eq!(s.pop(ActiveTarget::Xuannv), None);
    }
}
