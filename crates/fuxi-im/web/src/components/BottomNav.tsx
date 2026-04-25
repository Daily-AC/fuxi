import type { Component } from "solid-js";
import { A, useLocation } from "@solidjs/router";
import styles from "./BottomNav.module.css";

interface NavItem {
  href: string;
  label: string;
  match: (path: string) => boolean;
}

const items: NavItem[] = [
  { href: "/", label: "任务", match: (p) => p === "/" || p.startsWith("/task/") },
  { href: "/conv", label: "玄女", match: (p) => p === "/conv" },
];

// 底部导航：两枚 tab，每枚 ≥ 44px 触控热区。
// 暂只有"任务" / "玄女"两枚 —— 决策 14 心智模型确定。
// 不放铃铛 / 设置 / 我的 —— 单用户系统不需要。
export const BottomNav: Component = () => {
  const loc = useLocation();
  return (
    <nav class={styles.nav} role="navigation" aria-label="主导航">
      {items.map((it) => (
        <A
          href={it.href}
          class={styles.item}
          classList={{ [styles.active ?? ""]: it.match(loc.pathname) }}
          data-testid={`nav-${it.label}`}
        >
          <span class={styles.label}>{it.label}</span>
          <span class={styles.indicator} aria-hidden="true" />
        </A>
      ))}
    </nav>
  );
};
