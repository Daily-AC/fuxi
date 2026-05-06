import { For, type Component } from "solid-js";
import styles from "./BottomTabBar.module.css";

// Bottom tab bar · v3 #N1' / #36
//
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §A
//
// 心智：主流 IM (微信/Slack/Discord) 一致——固定 56px tab bar，5 项
// [玄女][任务][项目][交付][节点]，不允许 tab 间手势切换。
// Decision 21/22 phase 1 加 项目 / 交付 两个 tab。

export type TabIndex = 0 | 1 | 2 | 3 | 4 | 5;

export interface TabSpec {
  /** 内部 tab 标签，仅 testid + aria 用，不渲染。*/
  key: "xuannv" | "tasks" | "projects" | "deliverables" | "nodes" | "notifications";
  label: string;
  /** 红点 badge 数（>0 显，==0 不显）。任务 #9 hub 重构时通用化到所有 tab。 */
  badge?: number;
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
          const badge = (): number => t.badge ?? 0;
          return (
            <button
              type="button"
              class={styles.tab}
              role="tab"
              aria-selected={isActive()}
              aria-label={badge() > 0 ? `${t.label}（${badge()} 条未读）` : t.label}
              data-testid={`tab-${t.key}`}
              onClick={() => props.onChange(idx())}
            >
              <span class={styles.dot} aria-hidden="true" />
              <span class={styles.label}>{t.label}</span>
              {badge() > 0 && (
                <span
                  class={styles.badge}
                  data-testid={`tab-${t.key}-badge`}
                  aria-hidden="true"
                >
                  {badge() > 99 ? "99+" : badge()}
                </span>
              )}
            </button>
          );
        }}
      </For>
    </nav>
  );
};
