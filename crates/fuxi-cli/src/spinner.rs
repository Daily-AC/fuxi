//! Spinner · 三态（active / done / fail）单字符旋转指示器。
//!
//! 用于输入下沿"活状态行"（R6 ·#9）和 Toast（R3 ·#2）的进度标记：
//! - active：10 帧 braille，由宿主每 tick 驱动 `tick()` 推进
//! - done：静态 `✔`（green）
//! - fail：静态 `✕`（red）
//!
//! WHY 单字符 + Span：宿主把 Span inline 进自己的 Line，Spinner 不负责占位
//! 或换行——避免 widget 互相嵌 Block 产生的字符统计错位。
use ratatui::style::Style;
use ratatui::text::Span;

use crate::theme::Theme;

/// Braille 旋转帧——与 ora / yoctospinner 同序，视觉连贯不跳。
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Spinner 三态。
///
/// `Done` / `Fail` 是 α 预留给 toast 进度/结果场景的 surface（未来给
/// "任务完成 ✔" / "工具失败 ✕" 的通知用）——v1 REPL 活状态行只用 `Active`。
/// 保留这两态让后续接入无需改 enum 形状。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum SpinnerState {
    Active,
    Done,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Spinner {
    frames: &'static [&'static str],
    idx: usize,
    state: SpinnerState,
}

impl Spinner {
    pub fn new() -> Self {
        Self {
            frames: &FRAMES,
            idx: 0,
            state: SpinnerState::Active,
        }
    }

    /// 推进到下一帧。done/fail 下 no-op——静态字符不需要 tick。
    pub fn tick(&mut self) {
        if self.state == SpinnerState::Active {
            self.idx = (self.idx + 1) % self.frames.len();
        }
    }

    // v1 REPL 只用 Active 态；`mark_*` / `state` / `render` 是 α 预留 surface，
    // 接 toast 进度指示或 agent 完成通知时要用。保留 API 避免将来再改形状。
    #[allow(dead_code)]
    pub fn mark_done(&mut self) {
        self.state = SpinnerState::Done;
    }

    #[allow(dead_code)]
    pub fn mark_fail(&mut self) {
        self.state = SpinnerState::Fail;
    }

    #[allow(dead_code)]
    pub fn state(&self) -> SpinnerState {
        self.state
    }

    /// 当前字符——独立于主题，给不走主题的场景（日志）用。
    pub fn glyph(&self) -> &'static str {
        match self.state {
            SpinnerState::Active => self.frames[self.idx],
            SpinnerState::Done => "✔",
            SpinnerState::Fail => "✕",
        }
    }

    /// 渲染成着色 Span。active → 主题 info 色；done → success；fail → error。
    #[allow(dead_code)]
    pub fn render(&self, theme: &Theme) -> Span<'static> {
        let color = match self.state {
            SpinnerState::Active => theme.info(),
            SpinnerState::Done => theme.success(),
            SpinnerState::Fail => theme.error(),
        };
        Span::styled(self.glyph().to_string(), Style::default().fg(color))
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_cycles_through_frames() {
        let mut s = Spinner::new();
        let first = s.glyph();
        // 转一圈回到起点——10 次 tick。
        for _ in 0..FRAMES.len() {
            s.tick();
        }
        assert_eq!(s.glyph(), first, "10 tick 后应回到起始帧");

        // 半圈应落在中段不同字符——不然就是没前进。
        let mut s2 = Spinner::new();
        let first2 = s2.glyph();
        s2.tick();
        assert_ne!(s2.glyph(), first2, "tick 1 次后字符必须变化");
    }

    #[test]
    fn spinner_done_shows_tick() {
        let mut s = Spinner::new();
        s.mark_done();
        assert_eq!(s.glyph(), "✔");
        assert_eq!(s.state(), SpinnerState::Done);
    }

    #[test]
    fn spinner_fail_shows_cross() {
        let mut s = Spinner::new();
        s.mark_fail();
        assert_eq!(s.glyph(), "✕");
        assert_eq!(s.state(), SpinnerState::Fail);
    }

    #[test]
    fn tick_is_noop_after_done_or_fail() {
        let mut s = Spinner::new();
        s.mark_done();
        let g = s.glyph();
        s.tick();
        assert_eq!(s.glyph(), g, "done 态 tick 不应改变字符");

        let mut s = Spinner::new();
        s.mark_fail();
        let g = s.glyph();
        s.tick();
        assert_eq!(s.glyph(), g, "fail 态 tick 不应改变字符");
    }

    #[test]
    fn render_uses_theme_colors() {
        let theme = Theme::catppuccin_mocha();

        let s = Spinner::new();
        let span = s.render(&theme);
        assert_eq!(span.style.fg, Some(theme.info()));

        let mut s2 = Spinner::new();
        s2.mark_done();
        assert_eq!(s2.render(&theme).style.fg, Some(theme.success()));

        let mut s3 = Spinner::new();
        s3.mark_fail();
        assert_eq!(s3.render(&theme).style.fg, Some(theme.error()));
    }
}
