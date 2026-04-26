// 设计 token 单一源 · v2 PWA · task #16 spec
// 这是 TS 出口；CSS 同步在 src/styles/global.css。改 token 两边一起改。
// .impeccable.md 是源头，这里跟它对齐。

export const tokens = {
  // surfaces · 暖灰棕（Claude Code 风），不是冷黑
  bg: "#1F1E1B",
  surface: "#2A2925",
  surfaceElevated: "#33312D",
  border: "#3A3835",
  borderStrong: "#48453F",

  // text · 奶白米白
  textPrimary: "#F5F1E8",
  textSecondary: "#B8B0A0",
  textMuted: "#807868",

  // accent · Anthropic 橙
  accent: "#D97757",
  accentSubtle: "#3F2E26", // hover/active 暗底
  accentDim: "#3F2E26", // 兼容字段，指向同一暗调
  onAccent: "#1A0F08", // accent 底上的文字色，深棕，跟 #D97757 对比 ≥ 4.5:1

  // role 角色色（暖系协调）
  xuannv: "#C4A8E8",
  luban: "#E5A547",
  pusong: "#A0C277",

  // chat bubbles · 棕调暗底
  userBubble: "#3D332A",
  userBubbleText: "#F5F1E8",

  // semantic state
  success: "#88B47B",
  warning: "#E5B047",
  danger: "#D97777",

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
