// 设计 token 单一源 · v2 PWA · task #16 spec
// 这是 TS 出口；CSS 同步在 src/styles/global.css。改 token 两边一起改。
// .impeccable.md 是源头，这里跟它对齐。

export const tokens = {
  // surfaces
  bg: "#0a0a0a",
  surface: "#161616",
  surfaceElevated: "#1c1c1c",
  border: "#262626",
  borderStrong: "#333333",

  // text
  textPrimary: "#f4f4f5",
  textSecondary: "#a1a1aa",
  textMuted: "#71717a",

  // accent + role tints
  accent: "#22d3ee", // fuxi cyan
  accentDim: "#0e7490",
  xuannv: "#a78bfa",
  luban: "#fbbf24",
  pusong: "#34d399",

  // chat bubbles
  userBubble: "#1e3a5f",

  // semantic state
  success: "#10b981",
  warning: "#f59e0b",
  danger: "#ef4444",

  // typography
  fontSans: '-apple-system, BlinkMacSystemFont, "PingFang SC", "Noto Sans CJK SC", system-ui, sans-serif',
  fontMono: '"JetBrains Mono", "SF Mono", Menlo, Consolas, monospace',
  size: { meta: 11, aux: 13, body: 15, heading: 16, title: 18 },
  weight: { normal: 400, medium: 500, semibold: 600 },
  radius: { card: 12, bubble: 14, pill: 22 },

  // layout
  touch: 44,
  headerHeight: 52,
  composerHeight: 64,
} as const;

/** role → 颜色映射，用于消息组件按发言人色彩区分。
 *  unknown role 走 textSecondary（保守的灰）。*/
export function colorForRole(role: string | null | undefined): string {
  switch (role) {
    case "xuannv":
    case "玄女":
      return tokens.xuannv;
    case "luban":
    case "鲁班":
      return tokens.luban;
    case "pusong":
    case "蒲松":
      return tokens.pusong;
    default:
      return tokens.textSecondary;
  }
}
