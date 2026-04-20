//! 鼠标点击区域注册表——ratatui 不带 hit-testing 的手搓解法。
//!
//! 用法：
//! 1. 每帧 render 前调 `clear()`
//! 2. 每个可点击 widget 在画完后调 `register(rect, action)`
//! 3. crossterm `MouseEvent { column, row, .. }` 来时调 `hit_test(col, row)`
//!
//! 后注册的 region 覆盖先注册的（顶层 overlay 胜）。
//
// 设计背景：
// - ratatui 渲染是 immediate mode，widget 画完就忘——没有 widget tree 可供命中测试。
// - 社区成熟做法（ratatui-interact / rat-event）都是把 (Rect, Action) 外挂在一个 registry
//   里，渲染期注册、事件期查询。本模块保持最小集，不涉及焦点/悬停/拖拽——这些属于
//   上层状态机。
// - 泛型 `A: Clone` 让调用方既能存轻量 `enum Action`，也能存 `String`（如命令名）。
//   `hit_test` 返 `&A` 而非 `A`，避免强制 clone——调用方想 clone 自己决定。
//
// WHY `allow(dead_code)`：本模块作为独立基础设施先行落地，主线集成（`mod click_registry;`
// + 调用方）随后。移除这些 allow 的时机 = 调用方 use 起来后。

use ratatui::layout::Rect;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ClickRegistry<A: Clone> {
    regions: Vec<(Rect, A)>,
}

impl<A: Clone> Default for ClickRegistry<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl<A: Clone> ClickRegistry<A> {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }

    /// 每帧开始调——丢掉上帧 region。
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// 注册一个点击区。重叠时**后注册优先**（hit_test 逆序扫）。
    pub fn register(&mut self, rect: Rect, action: A) {
        self.regions.push((rect, action));
    }

    /// 返回 action 的引用；无命中时 `None`。包含性约定：x ∈ [x, x+w)，y ∈ [y, y+h)。
    pub fn hit_test(&self, column: u16, row: u16) -> Option<&A> {
        // 逆序：后注册的（画在顶层的 overlay）优先命中。
        self.regions
            .iter()
            .rev()
            .find(|(rect, _)| {
                column >= rect.x
                    && column < rect.x.saturating_add(rect.width)
                    && row >= rect.y
                    && row < rect.y.saturating_add(rect.height)
            })
            .map(|(_, action)| action)
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_inside_single_rect() {
        let mut reg: ClickRegistry<&'static str> = ClickRegistry::new();
        reg.register(Rect::new(2, 3, 5, 4), "hit");
        assert_eq!(reg.hit_test(3, 4), Some(&"hit"));
    }

    #[test]
    fn hit_test_on_boundary_is_exclusive_right_bottom() {
        // Rect::new(2, 3, 5, 4) 覆盖 x ∈ [2, 7)、y ∈ [3, 7)——最后一格是 (6, 6)。
        let mut reg: ClickRegistry<&'static str> = ClickRegistry::new();
        reg.register(Rect::new(2, 3, 5, 4), "hit");

        // 右下开区间——(7, 7) 越界不命中。
        assert_eq!(reg.hit_test(7, 7), None);
        // 左上闭区间——(2, 3) 命中。
        assert_eq!(reg.hit_test(2, 3), Some(&"hit"));
        // rect 最后一格 (6, 6) 命中。
        assert_eq!(reg.hit_test(6, 6), Some(&"hit"));
    }

    #[test]
    fn hit_test_outside_returns_none() {
        let mut reg: ClickRegistry<&'static str> = ClickRegistry::new();
        reg.register(Rect::new(2, 3, 5, 4), "hit");
        assert_eq!(reg.hit_test(0, 0), None);
    }

    #[test]
    fn hit_test_overlap_later_wins() {
        // 两个 rect 完全重叠——后注册的 "top" 胜。
        let mut reg: ClickRegistry<&'static str> = ClickRegistry::new();
        reg.register(Rect::new(0, 0, 10, 10), "bottom");
        reg.register(Rect::new(0, 0, 10, 10), "top");
        assert_eq!(reg.hit_test(5, 5), Some(&"top"));
    }

    #[test]
    fn clear_drops_regions() {
        let mut reg: ClickRegistry<&'static str> = ClickRegistry::new();
        reg.register(Rect::new(0, 0, 10, 10), "x");
        assert_eq!(reg.len(), 1);
        reg.clear();
        assert!(reg.is_empty());
        assert_eq!(reg.hit_test(5, 5), None);
    }

    #[test]
    fn generic_over_string_action() {
        let mut reg: ClickRegistry<String> = ClickRegistry::new();
        reg.register(Rect::new(1, 1, 3, 3), "open".to_string());
        assert_eq!(reg.hit_test(2, 2), Some(&"open".to_string()));
    }

    #[test]
    fn generic_over_enum_action() {
        #[derive(Debug, Clone, PartialEq, Eq)]
        enum TestAction {
            Focus(u32),
            Close,
        }

        let mut reg: ClickRegistry<TestAction> = ClickRegistry::new();
        reg.register(Rect::new(0, 0, 5, 5), TestAction::Focus(42));
        reg.register(Rect::new(10, 0, 3, 3), TestAction::Close);

        assert_eq!(reg.hit_test(2, 2), Some(&TestAction::Focus(42)));
        assert_eq!(reg.hit_test(11, 1), Some(&TestAction::Close));
        assert_eq!(reg.hit_test(8, 8), None);
    }
}
