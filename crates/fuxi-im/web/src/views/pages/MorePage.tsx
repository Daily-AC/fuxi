import { For, type Component } from "solid-js";
import { useApi, type MoreSubRoute } from "~/components/ApiProvider";
import styles from "./MorePage.module.css";

// 「更多」hub · v1-session17 task #9
//
// 设计 spec: docs/handoff/v1-session16.md §2.2 方案 A。
//
// 4 tab 模型（玄女 / 任务 / 通知 / 更多）下，原来的「项目 / 交付 / 节点」与
// 新的「记忆 / 角色 / 更漏」「工作者」「设置」全部进 hub 二级。tap 卡片
// setMoreSub(...) 让上层 App.tsx 渲染对应 sub-page；back-to-hub 由
// MoreSubShell 的返回按钮 setMoreSub(null) 处理。

interface Tile {
  sub: NonNullable<MoreSubRoute>;
  label: string;
  desc: string;
}

const TILES: Tile[] = [
  { sub: "nodes", label: "节点", desc: "home + 本地 dist topology" },
  { sub: "projects", label: "项目", desc: "L3 sandboxes + 注册项目" },
  { sub: "workers", label: "工作者", desc: "门客实例（v2 兼容入口）" },
  { sub: "deliverables", label: "交付物", desc: "门客交付收件箱" },
  { sub: "memory", label: "记忆", desc: "策府 oracle 现行事实" },
  { sub: "roles", label: "角色", desc: "门客 role 卡" },
  { sub: "cron", label: "更漏", desc: "scheduler trigger 列表" },
  { sub: "settings", label: "设置", desc: "推送 / 设备 / 关于" },
];

export const MorePage: Component = () => {
  const { setMoreSub } = useApi();
  return (
    <div class={styles.page} data-testid="page-more">
      <header class={styles.header}>
        <h1 class={styles.title}>更多</h1>
        <p class={styles.subtitle}>原一级 tab 与新增工具集中入口</p>
      </header>
      <div class={styles.body}>
        <div class={styles.grid}>
          <For each={TILES}>
            {(t) => (
              <button
                type="button"
                class={styles.tile}
                data-testid={`more-tile-${t.sub}`}
                onClick={() => setMoreSub(t.sub)}
                aria-label={`${t.label} · ${t.desc}`}
              >
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
