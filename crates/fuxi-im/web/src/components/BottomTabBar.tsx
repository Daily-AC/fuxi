import { For, type Component } from "solid-js";
import styles from "./BottomTabBar.module.css";

// Bottom tab bar · v3 #N1' / #36
//
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §A
//
// 心智：主流 IM (微信/Slack/Discord) 一致——固定 56px tab bar，3 项 [玄女][任务][节点]，
// 不允许 tab 间手势切换（vs v2 的 horizontal pager），active tab 橙色识别。

export type TabIndex = 0 | 1 | 2;

export interface TabSpec {
  /** 内部 tab 标签，仅 testid + aria 用，不渲染。*/
  key: "xuannv" | "tasks" | "nodes";
  label: string;
}

export interface BottomTabBarProps {
  tabs: TabSpec[];
  active: TabIndex;
  onChange(i: TabIndex): void;
}

export const BottomTabBar: Component<BottomTabBarProps> = (props) => {
  return (
    <nav class={styles.bar} role="tablist" data-testid="tab-bar">
      <For each={props.tabs}>
        {(t, i) => {
          const idx = (): TabIndex => i() as TabIndex;
          const isActive = (): boolean => idx() === props.active;
          return (
            <button
              type="button"
              class={styles.tab}
              role="tab"
              aria-selected={isActive()}
              aria-label={t.label}
              data-testid={`tab-${t.key}`}
              onClick={() => props.onChange(idx())}
            >
              <span class={styles.dot} aria-hidden="true" />
              <span class={styles.label}>{t.label}</span>
            </button>
          );
        }}
      </For>
    </nav>
  );
};
