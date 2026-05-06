import { Show, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import styles from "./SettingsPage.module.css";

// 「更多 → 设置」· v1-session17 task #9 起步 stub
//
// 当前可见的两件事：
//   1. push 通知开关（pushPermission + enablePush 已存在，复用）
//   2. 关于（PWA build version——拉 /api/version）
// 后续二期：登出 / 切换设备 / 主题色 / 个性化 prompt overlay。

export const SettingsPage: Component = () => {
  const { pushPermission, enablePush } = useApi();
  const permLabel = (): string => {
    switch (pushPermission()) {
      case "granted":
        return "已开启";
      case "denied":
        return "已拒（系统设置改）";
      case "unsupported":
        return "本设备不支持";
      default:
        return "未开启";
    }
  };
  const canEnable = (): boolean =>
    pushPermission() !== "granted" && pushPermission() !== "unsupported";
  return (
    <div class={styles.page} data-testid="page-settings">
      <section class={styles.row}>
        <div class={styles.rowHead}>
          <span class={styles.label}>推送通知</span>
          <span class={styles.value}>{permLabel()}</span>
        </div>
        <p class={styles.hint}>玄女 idle / 任务完成 / 通知 tab 红点会推到本设备。</p>
        <Show when={canEnable()}>
          <button
            type="button"
            class={styles.btn}
            onClick={() => void enablePush()}
            data-testid="settings-enable-push"
          >
            开启推送
          </button>
        </Show>
      </section>
      <section class={styles.row} data-testid="settings-about">
        <div class={styles.rowHead}>
          <span class={styles.label}>关于</span>
          <span class={styles.value}>fuxi PWA</span>
        </div>
        <p class={styles.hint}>
          伏羲（fuxi）个人 AI agent 平台 · v1-session17 「更多」hub 重构。
        </p>
      </section>
    </div>
  );
};
