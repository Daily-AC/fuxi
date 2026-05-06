import { For, type Component } from "solid-js";
import styles from "./BottomTabBar.module.css";

// Bottom tab bar · v1-session17 task #9 · 4 tab + 「更多」hub 重构
//
// 设计 spec: docs/handoff/v1-session16.md §2.2
//
// 心智：移动端 IM 主流（微信/Slack/Discord）一致——固定 56px tab bar，4 项
// [玄女][任务][通知][更多]，不允许 tab 间手势切换。
// 「项目」「交付」「节点」「工作者」「记忆」「角色」「更漏」「设置」全部进
// 「更多」hub 二级页面，PWA 内部 navPush 进入。
//
// 「通知」提一级是 first-class concern——每天看红点 badge 一眼知道有什么事
// 等我（玄女主动报的 bug + 门客审阅请求 + 上下文 handoff offer）。

export type TabIndex = 0 | 1 | 2 | 3;

export interface TabSpec {
  /** 内部 tab 标签，仅 testid + aria 用，不渲染。*/
  key: "xuannv" | "tasks" | "notifications" | "more";
  label: string;
  /** 红点 badge 数（>0 显，==0 不显）。通知 tab 用 unread_count；其他 tab 暂未用。*/
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
