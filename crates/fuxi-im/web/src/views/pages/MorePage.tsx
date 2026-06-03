import { For, type Component, type JSX } from "solid-js";
import { useApi, type MoreSubRoute } from "~/components/ApiProvider";
import styles from "./MorePage.module.css";

// 「更多」hub · v1-session17 task #9 · daimeng 奶油糖果重绘（archetype C · 宫格 hub）。
//
// 设计 spec: docs/handoff/v1-session16.md §2.2 方案 A。
//
// 4 tab 模型（玄女 / 任务 / 通知 / 更多）下，原来的「项目 / 交付 / 节点」与
// 新的「记忆 / 角色 / 更漏」「工作者」「设置」全部进 hub 二级。tap 卡片
// setMoreSub(...) 让上层 App.tsx 渲染对应 sub-page；back-to-hub 由
// MoreSubShell 的返回按钮 setMoreSub(null) 处理。
//
// RESKIN：保留全部行为 + data-testid（page-more / more-tile-<sub> 八张瓦片）。
// 视觉换共享原语：Tile 宫格瓦片（2 列），每张一渐变染色圆角 SVG 图标槽 + label + desc。
// 页底 u-mesh 暖光网格。禁 emoji——每瓦片一 inline SVG。

// 八张瓦片各自的 inline SVG（禁 emoji）。色由 iconSlot tone 按 data-tone 渲染。
const NodeIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <rect x="4" y="4" width="16" height="6" rx="2" />
    <rect x="4" y="14" width="16" height="6" rx="2" />
    <path d="M7.5 7h.01M7.5 17h.01" stroke-linecap="round" />
  </svg>
);

const ProjectIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path
      d="M3 7a2 2 0 0 1 2-2h4l2 2.5h6a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z"
      stroke-linejoin="round"
    />
  </svg>
);

const WorkerIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <circle cx="12" cy="8" r="3.4" />
    <path d="M5 19a7 7 0 0 1 14 0" stroke-linecap="round" />
  </svg>
);

const BoxIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M12 3 4 7v10l8 4 8-4V7l-8-4Z" stroke-linejoin="round" />
    <path d="M4 7l8 4 8-4M12 11v10" stroke-linejoin="round" />
  </svg>
);

const MemoryIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path
      d="M12 4a4 4 0 0 1 4 4 3.4 3.4 0 0 1 1 6.6A3.5 3.5 0 0 1 12 19a3.5 3.5 0 0 1-5-4.4A3.4 3.4 0 0 1 8 8a4 4 0 0 1 4-4Z"
      stroke-linejoin="round"
    />
    <path d="M12 4v15" stroke-linecap="round" />
  </svg>
);

const RoleIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <rect x="3.5" y="5" width="17" height="14" rx="2.5" />
    <circle cx="9" cy="11" r="2.2" />
    <path d="M5.5 16.5a3.5 3.5 0 0 1 7 0M14.5 9.5h3.5M14.5 13h3" stroke-linecap="round" />
  </svg>
);

const ClockIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <circle cx="12" cy="12" r="8.2" />
    <path d="M12 7.5V12l3 1.8" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);

const GearIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <circle cx="12" cy="12" r="3" />
    <path
      d="M12 2.5v2.2M12 19.3v2.2M21.5 12h-2.2M4.7 12H2.5M18.7 5.3l-1.6 1.6M6.9 17.1l-1.6 1.6M18.7 18.7l-1.6-1.6M6.9 6.9 5.3 5.3"
      stroke-linecap="round"
    />
  </svg>
);

interface Tile {
  sub: NonNullable<MoreSubRoute>;
  label: string;
  desc: string;
  icon: () => JSX.Element;
  // iconSlot 渐变染色调（plain peach / mint / lavender），让八瓦片有节奏感。
  tone: "plain" | "mint" | "lavender";
}

const TILES: Tile[] = [
  { sub: "nodes", label: "节点", desc: "home + 本地 dist topology", icon: NodeIcon, tone: "mint" },
  { sub: "projects", label: "项目", desc: "L3 sandboxes + 注册项目", icon: ProjectIcon, tone: "plain" },
  { sub: "workers", label: "工作者", desc: "门客实例（v2 兼容入口）", icon: WorkerIcon, tone: "lavender" },
  { sub: "deliverables", label: "交付物", desc: "门客交付收件箱", icon: BoxIcon, tone: "plain" },
  { sub: "memory", label: "记忆", desc: "策府 oracle 现行事实", icon: MemoryIcon, tone: "lavender" },
  { sub: "roles", label: "角色", desc: "门客 role 卡", icon: RoleIcon, tone: "mint" },
  { sub: "cron", label: "更漏", desc: "scheduler trigger 列表", icon: ClockIcon, tone: "plain" },
  { sub: "settings", label: "设置", desc: "推送 / 设备 / 关于", icon: GearIcon, tone: "lavender" },
];

export const MorePage: Component = () => {
  const { setMoreSub } = useApi();
  return (
    <div class={`u-mesh u-noise ${styles.page}`} data-testid="page-more">
      <header class={styles.header}>
        <h1 class={`u-title ${styles.title}`}>更多</h1>
        <p class={styles.subtitle}>原一级 tab 与新增工具集中入口</p>
      </header>
      <div class={styles.body}>
        <div class={styles.grid}>
          <For each={TILES}>
            {(t) => (
              <button
                type="button"
                class={`u-card ${styles.tile}`}
                data-testid={`more-tile-${t.sub}`}
                onClick={() => setMoreSub(t.sub)}
                aria-label={`${t.label} · ${t.desc}`}
              >
                <span class={styles.iconSlot} data-tone={t.tone} aria-hidden="true">
                  {t.icon()}
                </span>
                <span class={styles.tileLabel}>{t.label}</span>
                <span class={styles.tileDesc}>{t.desc}</span>
              </button>
            )}
          </For>
        </div>
      </div>
    </div>
  );
};
