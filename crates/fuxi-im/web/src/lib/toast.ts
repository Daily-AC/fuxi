// 全局 toast 信号 · 用于 4xx 等非 inline 错误（如 #N3 "门客正忙"）。
//
// 单实例：app shell 顶层挂 <Toast />，组件用 `pushToast(text, level?)` 推。
// kind=error 默认；info / warn 留给将来。
// 自动 N 秒后消失（默认 3.2s），新 push 替换旧 toast（不堆叠，避免移动端遮内容）。

import { createSignal, type Accessor } from "solid-js";

export type ToastLevel = "info" | "warn" | "error";

export interface Toast {
  id: number;
  text: string;
  level: ToastLevel;
}

const [current, setCurrent] = createSignal<Toast | null>(null);
let nextId = 1;
let dismissTimer: ReturnType<typeof setTimeout> | null = null;
const DEFAULT_TTL_MS = 3200;

export const currentToast: Accessor<Toast | null> = current;

export function pushToast(text: string, level: ToastLevel = "error", ttlMs = DEFAULT_TTL_MS): void {
  const t: Toast = { id: nextId++, text, level };
  setCurrent(t);
  if (dismissTimer) clearTimeout(dismissTimer);
  dismissTimer = setTimeout(() => {
    setCurrent((cur) => (cur && cur.id === t.id ? null : cur));
    dismissTimer = null;
  }, ttlMs);
}

export function dismissToast(): void {
  if (dismissTimer) {
    clearTimeout(dismissTimer);
    dismissTimer = null;
  }
  setCurrent(null);
}
