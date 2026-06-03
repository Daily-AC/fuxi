// 设计 token 单一源 · 奶油糖果重设计 v3 PWA
// 这是 TS 出口；CSS 同步在 src/styles/global.css（将在后续 task 同步）。

export const tokens = {
  bg: "#FFF8F0",
  surface: "#FFFFFF",
  surfaceSoft: "#FFFDFA",
  surfaceElevated: "#FFFFFF",
  border: "#F0E2D2",
  borderStrong: "#E6D5C0",

  textPrimary: "#5A4A3A",
  textSecondary: "#9A8C7A",
  textMuted: "#C3B4A2",

  accent: "#FFB877",
  accentDeep: "#E8915A",
  accentSubtle: "#FFF0DC",
  // 兼容旧字段 accentDim（旧暗底 → 奶油版映射到 accentSubtle 的值）
  accentDim: "#FFF0DC",
  onAccent: "#FFFFFF",

  mint: "#8FD9B6",
  mintSoft: "#A8E6CF",
  lavender: "#C9B6E8",
  pink: "#FF9EB5",

  xuannv: "#C9B6E8",
  luban: "#E8915A",
  pusong: "#6FB893",

  userBubble: "#FFFFFF",
  userBubbleText: "#5A4A3A",

  success: "#6FB893",
  warning: "#E8A23A",
  danger: "#E8857A",

  fontSans: '"Yuanti SC", -apple-system, BlinkMacSystemFont, "PingFang SC", "Noto Sans CJK SC", system-ui, sans-serif',
  fontRound: '"Yuanti SC", "PingFang SC", system-ui, sans-serif',
  fontMono: '"JetBrains Mono", "SF Mono", Menlo, Consolas, monospace',
  size: { meta: 11, aux: 13, body: 15, heading: 16, title: 19 },
  weight: { normal: 400, medium: 500, semibold: 700 },
  radius: {
    card: 20,
    bubble: 20,  // 兼容旧字段（旧值 14 → 新奶油风 20）
    sheet: 28,
    pill: 999,
    chip: 14,
  },

  touch: 44,
  headerHeight: 54,
  composerHeight: 64,
} as const;

export function colorForRole(role: string | null | undefined): string {
  switch (role) {
    case "xuannv": case "玄女": return tokens.xuannv;
    case "luban": case "鲁班": return tokens.luban;
    case "pusong": case "蒲松": return tokens.pusong;
    default: return tokens.textSecondary;
  }
}
