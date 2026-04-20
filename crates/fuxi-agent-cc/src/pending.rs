//! 消息黑洞修（M2.1）的核心数据结构。
//!
//! ## 为什么独立成模块
//!
//! `send_message` 在 cc 处于 tool loop 的 turn 中时直接 `channel.send` 会被 cc
//! 丢弃（cc 不 poll WS 直到当前 turn 结束）。修法是**入队**——busy 时存到
//! `PendingOutbox`，pump 在 turn terminal（`ResultSuccess`/`ResultError`）处
//! drain 后按 FIFO send。
//!
//! 抽出独立 struct 的目的：纯逻辑可单测——不必 spawn cc 子进程，TDD 三条直接
//! 覆盖入队、FIFO drain、overflow 驱逐 + 信号上报。

use std::collections::VecDeque;

/// 默认队列上限。32 条已经够覆盖"用户 busy 时连按几次回车"的洪峰；
/// 再多基本是误用，应让上层显式打断。
pub const DEFAULT_PENDING_CAP: usize = 32;

/// 入队结果——上层据此判断是否需要 emit `AgentInterrupted` 信号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// 正常入队。
    Queued,
    /// 队列已满，**最旧**的一条被驱逐——上层应发 `AgentInterrupted`
    /// `{ reason: "queue_overflow" }` 让玄女知情。
    Overflowed {
        /// 被驱逐的旧消息（保留供日志/审计；后续如要重投可用）。
        dropped: String,
    },
}

/// 单 agent 的 pending 出箱队列。FIFO，硬上限。
#[derive(Debug)]
pub struct PendingOutbox {
    queue: VecDeque<String>,
    cap: usize,
}

impl PendingOutbox {
    /// 用默认上限新建。
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_PENDING_CAP)
    }

    /// 自定义上限——主要给单测用。
    pub fn with_capacity(cap: usize) -> Self {
        debug_assert!(cap > 0, "pending cap must be > 0");
        Self {
            queue: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// 入队一条文本；满时驱逐**队首**（最旧）并返回它。
    pub fn enqueue(&mut self, text: String) -> EnqueueOutcome {
        if self.queue.len() >= self.cap {
            // 先弹再 push，保证队列大小恒等于 cap，且新 message 留在队尾
            // （FIFO drain 时是最后才被处理的，符合"老消息先丢"语义）。
            let dropped = self
                .queue
                .pop_front()
                .expect("len >= cap > 0 implies non-empty");
            self.queue.push_back(text);
            EnqueueOutcome::Overflowed { dropped }
        } else {
            self.queue.push_back(text);
            EnqueueOutcome::Queued
        }
    }

    /// 清空并按 FIFO 顺序返回全部 pending message——上层在 turn terminal 处调用，
    /// 然后逐条 `channel.send` 到 cc。
    pub fn drain_all(&mut self) -> Vec<String> {
        self.queue.drain(..).collect()
    }

    /// 当前 pending 数量。
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 是否空——`turn_done` 处可早 return 避免无意义遍历。
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl Default for PendingOutbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TDD #1：busy 时连发 3 条都进队，**一条不丢**。
    /// 等价于 send_message_while_busy_queues_not_drops——把 send_message 抽象
    /// 到 enqueue 之后，busy 路径就是这个 enqueue 调用本身。
    #[test]
    fn send_message_while_busy_queues_not_drops() {
        let mut outbox = PendingOutbox::new();
        assert_eq!(outbox.enqueue("m1".into()), EnqueueOutcome::Queued);
        assert_eq!(outbox.enqueue("m2".into()), EnqueueOutcome::Queued);
        assert_eq!(outbox.enqueue("m3".into()), EnqueueOutcome::Queued);
        assert_eq!(outbox.len(), 3);
    }

    /// TDD #2：turn 结束后 drain 必须按 FIFO 顺序——cc 收到的应是用户原始打字次序。
    #[test]
    fn pending_drains_after_turn_done_in_fifo_order() {
        let mut outbox = PendingOutbox::new();
        for i in 0..5 {
            outbox.enqueue(format!("msg-{i}"));
        }
        let drained = outbox.drain_all();
        assert_eq!(
            drained,
            vec!["msg-0", "msg-1", "msg-2", "msg-3", "msg-4"],
            "drain 必须保 FIFO"
        );
        assert!(outbox.is_empty(), "drain 后队列应清空");
    }

    /// TDD #3：超过 cap 时驱逐**最老**的一条；返回值带上被驱逐的 text 让上层
    /// 用它发 AgentInterrupted 事件。
    #[test]
    fn queue_overflow_emits_interrupted_event_and_drops_oldest() {
        // cap=3 测得快——语义和 cap=32 一致
        let mut outbox = PendingOutbox::with_capacity(3);
        assert_eq!(outbox.enqueue("a".into()), EnqueueOutcome::Queued);
        assert_eq!(outbox.enqueue("b".into()), EnqueueOutcome::Queued);
        assert_eq!(outbox.enqueue("c".into()), EnqueueOutcome::Queued);
        // 第 4 条触发 overflow：最老的 "a" 被驱逐
        assert_eq!(
            outbox.enqueue("d".into()),
            EnqueueOutcome::Overflowed {
                dropped: "a".into()
            }
        );
        // 队列大小恒为 cap，且尾部是新进来的 "d"
        assert_eq!(outbox.len(), 3);
        let drained = outbox.drain_all();
        assert_eq!(
            drained,
            vec!["b", "c", "d"],
            "overflow 后剩余顺序应是 b/c/d——a 已被驱逐，d 尾插"
        );
    }

    /// 防退化：同一 outbox enqueue → drain → 再 enqueue 应可继续工作。
    /// （隐含 bug：若实现误把 cap 用 set_len 之类的硬截断，会在第二轮失常。）
    #[test]
    fn outbox_is_reusable_across_drain_cycles() {
        let mut outbox = PendingOutbox::with_capacity(2);
        outbox.enqueue("x".into());
        outbox.enqueue("y".into());
        assert_eq!(outbox.drain_all(), vec!["x", "y"]);
        assert_eq!(outbox.enqueue("z".into()), EnqueueOutcome::Queued);
        assert_eq!(outbox.drain_all(), vec!["z"]);
    }
}
