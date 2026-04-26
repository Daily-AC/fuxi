import type { Component } from "solid-js";
import styles from "./Header.module.css";

// 顶栏 · 三 tap target：任务（左）/ 玄女 + 在线状态（中）/ 节点（右）。
// 不放头像、不放搜索 —— 单用户；副视图通过 sheet 召唤而非新页面。
export interface HeaderProps {
  online: boolean;
  onOpenTasks: () => void;
  onOpenNodes: () => void;
}

export const Header: Component<HeaderProps> = (props) => {
  return (
    <header class={styles.header} role="banner">
      <button
        class={styles.side}
        type="button"
        onClick={() => props.onOpenTasks()}
        data-testid="header-tasks"
        aria-label="任务"
      >
        任务
      </button>
      <div class={styles.center} data-testid="header-center">
        <div class={styles.title}>玄女</div>
        <div class={styles.statusRow}>
          <span
            class={styles.dot}
            classList={{ [styles.dotOn ?? ""]: props.online }}
            aria-hidden="true"
          />
          <span class={styles.status}>{props.online ? "在线" : "重连中"}</span>
        </div>
      </div>
      <button
        class={styles.side}
        type="button"
        onClick={() => props.onOpenNodes()}
        data-testid="header-nodes"
        aria-label="节点"
      >
        节点
      </button>
    </header>
  );
};
