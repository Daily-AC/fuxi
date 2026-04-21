//! Toast · 右上角浮层通知栈。
//!
//! 四档语义：info / success / warn / error → 映射到主题色（见 Theme）。
//! 宿主（repl）push 通知 → 每 render 帧 prune 过期 → render 得一组 (Rect, Paragraph)
//! overlay 到 main frame 上。
//!
//! WHY 栈上限 5 条：终端高度有限，叠太多自身就变干扰。FIFO 淘汰最旧的。
//! WHY render 返回 Vec<(Rect, Paragraph)>：Toast 不持有 frame，宿主来 draw；
//! 这样无须把宿主的 Frame 泄漏进模块。
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme::Theme;

/// Toast 最多同屏挂 5 条——超过就淘汰最旧的。
/// WHY 5：Catppuccin 终端主流高度 24-40 行，5 条右上占 ~5 行 overlay 感觉刚好。
pub const TOAST_MAX: usize = 5;

/// Toast 的语义类型——决定配色。
///
/// `Info` / `Warn` v1 REPL 暂未主动 push（现阶段只发 Success/Error），
/// 但保留 4 档语义完整性——未来加"粘贴成功/切题警告"等场景无须改 enum。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ToastVariant {
    Info,
    Success,
    Warn,
    Error,
}

impl ToastVariant {
    /// 映射到主题语义色。变更配色**一律改 Theme 别名**，别在此硬编。
    pub fn color(self, theme: &Theme) -> ratatui::style::Color {
        match self {
            ToastVariant::Info => theme.info(),
            ToastVariant::Success => theme.success(),
            ToastVariant::Warn => theme.warn(),
            ToastVariant::Error => theme.error(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub variant: ToastVariant,
    pub created_at: Instant,
    pub ttl: Duration,
}

impl Toast {
    pub fn new(text: impl Into<String>, variant: ToastVariant, ttl: Duration) -> Self {
        Self {
            text: text.into(),
            variant,
            created_at: Instant::now(),
            ttl,
        }
    }

    /// 指定 `created_at` 构造——测试时用 Instant::now() 做 anchor。
    #[allow(dead_code)] // 仅测试用：确定性 TTL 判定需要固定锚点
    pub fn with_created_at(
        text: impl Into<String>,
        variant: ToastVariant,
        ttl: Duration,
        created_at: Instant,
    ) -> Self {
        Self {
            text: text.into(),
            variant,
            created_at,
            ttl,
        }
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created_at) >= self.ttl
    }
}

#[derive(Debug, Default)]
pub struct ToastStack {
    toasts: Vec<Toast>,
}

impl ToastStack {
    pub fn new() -> Self {
        Self::default()
    }

    // 宿主 introspection / 测试辅助——非宿主 hot path。保留以便未来做
    // "当前未读通知计数"或 Firehose 观察窗。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.toasts.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }

    /// 压入一条新 toast；超过 `TOAST_MAX` 时淘汰最旧的。
    pub fn push(&mut self, text: impl Into<String>, variant: ToastVariant, ttl: Duration) {
        self.toasts.push(Toast::new(text, variant, ttl));
        while self.toasts.len() > TOAST_MAX {
            self.toasts.remove(0);
        }
    }

    /// 按"now"清掉过期的——宿主 render 帧首调一次。
    pub fn prune(&mut self, now: Instant) {
        self.toasts.retain(|t| !t.is_expired(now));
    }

    /// 渲染为 overlay 所需的 (Rect, Paragraph) 列表。
    ///
    /// 布局：右上角堆叠，每条占 1 行（单行 Paragraph 带 Borders::NONE），
    /// 宽度 = min(text_width + padding, area.width / 2)。
    /// WHY 不带边框：Toast 是临时提示，边框会抢视觉；靠颜色区分足矣。
    pub fn render(&self, theme: &Theme, area: Rect) -> Vec<(Rect, Paragraph<'static>)> {
        if self.toasts.is_empty() || area.width < 4 || area.height < 1 {
            return Vec::new();
        }

        let max_width = area
            .width
            .saturating_sub(2)
            .min(area.width / 2 + 20)
            .max(10);
        let mut out = Vec::with_capacity(self.toasts.len());
        // 最新的放最上面——视觉直觉：新消息抢眼。
        for (i, toast) in self.toasts.iter().rev().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            // 宽度：取字符数 + 2 padding，夹到 [10, max_width]。
            let shown_chars = toast.text.chars().count() as u16;
            let w = (shown_chars + 2).clamp(10, max_width);
            let x = area.x + area.width.saturating_sub(w + 1); // 右贴边留 1 列缝

            let rect = Rect {
                x,
                y,
                width: w,
                height: 1,
            };

            let color = toast.variant.color(theme);
            let line = Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(toast.text.clone(), Style::default().fg(color)),
            ]);
            let para = Paragraph::new(line).block(Block::default().borders(Borders::NONE));
            out.push((rect, para));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_expires_after_ttl() {
        let anchor = Instant::now();
        let toast =
            Toast::with_created_at("hi", ToastVariant::Info, Duration::from_millis(100), anchor);
        assert!(!toast.is_expired(anchor));
        assert!(!toast.is_expired(anchor + Duration::from_millis(50)));
        assert!(toast.is_expired(anchor + Duration::from_millis(100)));
        assert!(toast.is_expired(anchor + Duration::from_millis(200)));
    }

    #[test]
    fn toast_variant_maps_to_theme_color() {
        let theme = Theme::catppuccin_mocha();
        assert_eq!(ToastVariant::Info.color(&theme), theme.info());
        assert_eq!(ToastVariant::Success.color(&theme), theme.success());
        assert_eq!(ToastVariant::Warn.color(&theme), theme.warn());
        assert_eq!(ToastVariant::Error.color(&theme), theme.error());
    }

    #[test]
    fn toast_stack_max_5() {
        let mut stack = ToastStack::new();
        for i in 0..10 {
            stack.push(
                format!("msg {i}"),
                ToastVariant::Info,
                Duration::from_secs(60),
            );
        }
        assert_eq!(stack.len(), TOAST_MAX);
        // 保留的是最新的 5 条——最旧的被淘汰。
        let texts: Vec<_> = stack.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, vec!["msg 5", "msg 6", "msg 7", "msg 8", "msg 9"]);
    }

    #[test]
    fn prune_removes_expired_keeps_fresh() {
        let anchor = Instant::now();
        let mut stack = ToastStack::default();
        // 手工填 3 条不同 ttl。
        stack.toasts.push(Toast::with_created_at(
            "old",
            ToastVariant::Info,
            Duration::from_millis(10),
            anchor,
        ));
        stack.toasts.push(Toast::with_created_at(
            "fresh",
            ToastVariant::Warn,
            Duration::from_secs(60),
            anchor,
        ));
        stack.toasts.push(Toast::with_created_at(
            "just_dead",
            ToastVariant::Error,
            Duration::from_millis(50),
            anchor,
        ));

        stack.prune(anchor + Duration::from_millis(100));
        let texts: Vec<_> = stack.iter().map(|t| t.text.clone()).collect();
        assert_eq!(texts, vec!["fresh"]);
    }

    #[test]
    fn render_empty_stack_returns_empty() {
        let theme = Theme::catppuccin_mocha();
        let stack = ToastStack::new();
        let rects = stack.render(
            &theme,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        assert!(rects.is_empty());
    }

    #[test]
    fn render_returns_one_rect_per_toast() {
        let theme = Theme::catppuccin_mocha();
        let mut stack = ToastStack::new();
        stack.push("a", ToastVariant::Info, Duration::from_secs(10));
        stack.push("bb", ToastVariant::Error, Duration::from_secs(10));

        let rects = stack.render(
            &theme,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
        );
        assert_eq!(rects.len(), 2);
        // 所有矩形都落在 area 内。
        for (r, _) in &rects {
            assert!(r.x + r.width <= 80);
            assert!(r.y < 24);
        }
    }
}
