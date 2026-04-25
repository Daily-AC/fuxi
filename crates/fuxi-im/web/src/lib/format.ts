// 时间 / agent id 显示规则。中文优先，简短 hash。
export function shortAgentId(id: string): string {
  if (id.length <= 8) return id;
  return `${id.slice(0, 4)}-${id.slice(-4)}`;
}

export function relativeTime(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const diff = now.getTime() - d.getTime();
  const sec = Math.round(diff / 1000);
  if (sec < 0) return "刚刚";
  if (sec < 60) return "刚刚";
  const min = Math.round(sec / 60);
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  const day = Math.round(hr / 24);
  if (day < 7) return `${day} 天前`;
  return d.toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
}

export function statusLabel(s: string): string {
  return (
    {
      pending: "待派",
      running: "进行中",
      done: "完成",
      failed: "失败",
      blocked: "阻塞",
    }[s] ?? s
  );
}
