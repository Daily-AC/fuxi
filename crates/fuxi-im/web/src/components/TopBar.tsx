import type { Component } from "solid-js";
import { useLocation } from "@solidjs/router";
import { useApi } from "./ApiProvider";
import styles from "./TopBar.module.css";

// 顶栏：左侧标题，右侧推送状态指示。
// 不放头像、不放搜索 —— 单用户不需要。
export const TopBar: Component = () => {
  const loc = useLocation();
  const { pushPermission, enablePush } = useApi();

  const title = () => {
    const path = loc.pathname;
    if (path.startsWith("/task/")) return "任务";
    if (path === "/conv") return "玄女";
    return "伏羲";
  };

  return (
    <header class={styles.bar} role="banner">
      <div class={styles.title}>{title()}</div>
      <button
        class={styles.pushBtn}
        classList={{
          [styles.pushOn ?? ""]: pushPermission() === "granted",
          [styles.pushOff ?? ""]:
            pushPermission() === "denied" || pushPermission() === "unsupported",
        }}
        aria-label="推送通知"
        onClick={() => {
          if (pushPermission() !== "granted") void enablePush();
        }}
      >
        <span class={styles.dot} aria-hidden="true" />
        <span class={styles.pushLabel}>
          {pushPermission() === "granted"
            ? "推送已开"
            : pushPermission() === "denied"
              ? "推送已拒"
              : pushPermission() === "unsupported"
                ? "不支持"
                : "开启推送"}
        </span>
      </button>
    </header>
  );
};
