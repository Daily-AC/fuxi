import { For, type Component, type JSX } from "solid-js";
import styles from "./BottomTabBar.module.css";

// Bottom tab bar · daimeng 奶油糖果重构 · 5 tab + 毛玻璃 + SVG 图标
//
// tab 模型：[聊天][任务][家][通知][更多]，不允许 tab 间手势切换。
// 「通知」重新升为一级 tab（带 unread 红点 badge）。
// 「项目」「交付」「节点」「工作者」「记忆」「角色」「更漏」「设置」全部进
// 「更多」hub 二级页面，PWA 内部 navPush 进入。
//
// 图标：纯 inline SVG（无 emoji），currentColor 描边——选中走 --accent-deep，
// 未选中走 --text-muted，靠 CSS 切换。

export type TabIndex = 0 | 1 | 2 | 3 | 4;

export type TabKey = "xuannv" | "notifications" | "home" | "tasks" | "more";

export interface TabSpec {
  /** 内部 tab 标签，testid + aria + 图标分发用，不渲染文本。*/
  key: TabKey;
  label: string;
  /** 红点 badge 数（>0 显，==0 不显）。通知 tab 用 unread_count；其他 tab 暂未用。*/
  badge?: number;
}

export interface BottomTabBarProps {
  tabs: TabSpec[];
  active: TabIndex;
  onChange(i: TabIndex): void;
}

// 各 tab 的 inline SVG 图标 · ~20px · currentColor 描边 · round linecap。
// data-testid="tab-<key>-icon" 给单测定位。
function tabIcon(key: TabKey): JSX.Element {
  const common = {
    width: "20",
    height: "20",
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    "stroke-width": "1.8",
    "stroke-linecap": "round" as const,
    "stroke-linejoin": "round" as const,
    "aria-hidden": true,
  };
  switch (key) {
    case "home":
      // 家 tab 用小玄女头像（圆形裁剪自 idle 帧）替代纯线条房子图标——让「家」更显眼、
      // 跟玄女这个产品主体绑定。img 不吃 currentColor，选中/未选的反馈靠 CSS 调
      // 透明度 + accent 描边圈（见 .homeMascot）。
      return (
        <img
          class={styles.homeMascot}
          src="/mascot/xuannv-idle.webp"
          alt=""
          aria-hidden="true"
          data-testid="tab-home-icon"
        />
      );
    case "xuannv":
      return (
        <svg {...common} data-testid="tab-xuannv-icon">
          <path d="M5 5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H9l-4 3v-3a1 1 0 0 1 0 0V6a1 1 0 0 1 1-1Z" />
        </svg>
      );
    case "notifications":
      return (
        <svg {...common} data-testid="tab-notifications-icon">
          <path d="M6 16V10.5a6 6 0 0 1 12 0V16l1.5 2.5h-15L6 16Z" />
          <path d="M10 19.5a2 2 0 0 0 4 0" />
        </svg>
      );
    case "tasks":
      return (
        <svg {...common} data-testid="tab-tasks-icon">
          <path d="m4 7 1.5 1.5L8 6" />
          <path d="m4 16 1.5 1.5L8 15" />
          <path d="M11 7h9" />
          <path d="M11 16h9" />
        </svg>
      );
    case "more":
      return (
        <svg {...common} data-testid="tab-more-icon">
          <rect x="4" y="4" width="7" height="7" rx="1.5" />
          <rect x="13" y="4" width="7" height="7" rx="1.5" />
          <rect x="4" y="13" width="7" height="7" rx="1.5" />
          <rect x="13" y="13" width="7" height="7" rx="1.5" />
        </svg>
      );
  }
}

export const BottomTabBar: Component<BottomTabBarProps> = (props) => {
  return (
    <nav class={"u-glass " + styles.bar} role="tablist" data-testid="tab-bar">
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
              <span class={styles.icon}>{tabIcon(t.key)}</span>
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
