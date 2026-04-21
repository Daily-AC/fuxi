//! Prompt history · 上下方向键翻旧输入。
//!
//! 数据结构：固定容量环（VecDeque）。`push` 加到末尾；满了丢最旧。
//! 光标语义：
//! - `None` = "实时"（用户正在编辑当前未提交的草稿）
//! - `Some(i)` = 指向 `ring[i]`（0 最旧 → len-1 最新）
//! - `up()` 从 None 进入 len-1（最新提交过的）；再按继续向旧翻
//! - `down()` 向新翻；翻到最新再 down → 回到 None（草稿层）
//!
//! WHY 环 + 光标：无状态 struct 没法支持"离开又回来"的语义——cursor 是
//! 用户体感的连续性锚点。
use std::collections::VecDeque;

/// 默认容量：100。够一次会话翻看不浪费内存。
pub const DEFAULT_CAP: usize = 100;

#[derive(Debug)]
pub struct PromptHistory {
    ring: VecDeque<String>,
    cap: usize,
    cursor: Option<usize>,
}

impl PromptHistory {
    pub fn new(cap: usize) -> Self {
        assert!(cap > 0, "cap 必须 > 0");
        Self {
            ring: VecDeque::with_capacity(cap),
            cap,
            cursor: None,
        }
    }

    // 宿主未主动用 len/is_empty——测试需要；未来 footer 可显示"历史 N 条"。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// 记录一条已提交的输入。
    ///
    /// WHY 连续去重：连续提交相同内容不存第二条——bash/zsh 约定俗成。
    /// 容量满 → 丢最旧。push 后光标 **重置**——用户刚提交完，逻辑上回到"实时"。
    pub fn push(&mut self, s: impl Into<String>) {
        let s = s.into();
        if s.is_empty() {
            return;
        }
        if let Some(last) = self.ring.back()
            && last == &s
        {
            self.reset_cursor();
            return;
        }
        self.ring.push_back(s);
        while self.ring.len() > self.cap {
            self.ring.pop_front();
        }
        self.reset_cursor();
    }

    /// 上翻。None → len-1；Some(i) → i.saturating_sub(1) 但不 < 0。
    /// 返回光标指向的项；空 ring / 已在最旧时返回 None。
    pub fn up(&mut self) -> Option<&str> {
        if self.ring.is_empty() {
            return None;
        }
        self.cursor = match self.cursor {
            None => Some(self.ring.len() - 1),
            Some(0) => Some(0), // 已在最旧，保留
            Some(i) => Some(i - 1),
        };
        self.cursor
            .and_then(|i| self.ring.get(i).map(|s| s.as_str()))
    }

    /// 下翻。None → None；Some(len-1) → None（回到实时）；Some(i) → Some(i+1)。
    pub fn down(&mut self) -> Option<&str> {
        let cursor = self.cursor?;
        if cursor + 1 >= self.ring.len() {
            self.cursor = None;
            return None;
        }
        self.cursor = Some(cursor + 1);
        self.ring.get(cursor + 1).map(|s| s.as_str())
    }

    /// 显式离开历史导航——回到"实时"层。
    pub fn reset_cursor(&mut self) {
        self.cursor = None;
    }
}

impl Default for PromptHistory {
    fn default() -> Self {
        Self::new(DEFAULT_CAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_cap_evicts_oldest() {
        let mut h = PromptHistory::new(3);
        h.push("a");
        h.push("b");
        h.push("c");
        h.push("d");
        assert_eq!(h.len(), 3);
        assert_eq!(
            h.ring.iter().cloned().collect::<Vec<_>>(),
            vec!["b", "c", "d"]
        );
    }

    #[test]
    fn up_cycles_backward() {
        let mut h = PromptHistory::new(10);
        h.push("one");
        h.push("two");
        h.push("three");

        // 从实时起连上 3 次：three → two → one；第 4 次停在 one。
        assert_eq!(h.up(), Some("three"));
        assert_eq!(h.up(), Some("two"));
        assert_eq!(h.up(), Some("one"));
        assert_eq!(h.up(), Some("one"), "已到最旧应卡住不再前进");
    }

    #[test]
    fn reset_returns_to_live() {
        let mut h = PromptHistory::new(10);
        h.push("x");
        h.push("y");

        h.up(); // now on y
        h.up(); // now on x
        assert_eq!(h.cursor(), Some(0));
        h.reset_cursor();
        assert_eq!(h.cursor(), None);
    }

    #[test]
    fn down_from_newest_returns_to_live() {
        let mut h = PromptHistory::new(10);
        h.push("x");
        h.push("y");
        h.up(); // y (cursor=1)
        assert_eq!(h.cursor(), Some(1));
        assert_eq!(h.down(), None, "到最新再下 = 回实时");
        assert_eq!(h.cursor(), None);
    }

    #[test]
    fn down_midway_advances() {
        let mut h = PromptHistory::new(10);
        h.push("a");
        h.push("b");
        h.push("c");

        h.up(); // c
        h.up(); // b
        h.up(); // a
        assert_eq!(h.down(), Some("b"));
        assert_eq!(h.down(), Some("c"));
        assert_eq!(h.down(), None); // 实时
    }

    #[test]
    fn push_ignores_consecutive_duplicates() {
        let mut h = PromptHistory::new(10);
        h.push("same");
        h.push("same");
        h.push("same");
        assert_eq!(h.len(), 1, "连续重复应去重");
        h.push("other");
        h.push("same");
        assert_eq!(h.len(), 3, "非连续重复仍保留");
    }

    #[test]
    fn push_resets_cursor() {
        let mut h = PromptHistory::new(10);
        h.push("x");
        h.up();
        assert_eq!(h.cursor(), Some(0));
        h.push("y");
        assert_eq!(h.cursor(), None, "新 push 应把光标拉回实时");
    }

    #[test]
    fn empty_history_returns_none() {
        let mut h = PromptHistory::new(10);
        assert_eq!(h.up(), None);
        assert_eq!(h.down(), None);
    }

    #[test]
    fn push_ignores_empty_string() {
        let mut h = PromptHistory::new(10);
        h.push("");
        assert_eq!(h.len(), 0);
    }
}
