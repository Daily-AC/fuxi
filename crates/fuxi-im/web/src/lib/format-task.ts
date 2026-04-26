// 任务相关 format 工具 · 阶段 4。
import { tokens } from "~/tokens";

/** 把 duration_ms 格式化为 "0:12" / "3:20" / "1:23:45"。*/
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0:00";
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) {
    return `${h}:${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
  }
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** tokens 数字 → "1.2k" / "12.3k" / "123" 人性化。0 / null 视为"无值"返 ""。*/
export function formatTokens(n: number | null | undefined): string {
  if (typeof n !== "number" || !Number.isFinite(n) || n <= 0) return "";
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(1)}k`;
  return `${Math.round(n / 1000)}k`;
}

/** task id 短化：取 #后8 位。如果有 # 前缀就保留。*/
export function shortTaskId(id: string): string {
  if (!id) return "#?";
  const tail = id.length > 8 ? id.slice(-8) : id;
  return `#${tail}`;
}

/** role key → 颜色（用 token 里的 role tints）。*/
export function colorForTaskRole(role: string): string {
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
