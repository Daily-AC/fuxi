import { Show, type Component } from "solid-js";
import { currentToast, dismissToast } from "~/lib/toast";
import styles from "./Toast.module.css";

// 全局 toast · App shell 顶层挂一个，组件用 lib/toast::pushToast 触发。
// 单 toast，不堆叠（移动端遮挡 + 用户认知负担）。
export const Toast: Component = () => {
  return (
    <Show when={currentToast()}>
      {(t) => (
        <div
          class={styles.toast}
          classList={{
            [styles.error ?? ""]: t().level === "error",
            [styles.warn ?? ""]: t().level === "warn",
            [styles.info ?? ""]: t().level === "info",
          }}
          role="status"
          aria-live="polite"
          data-testid="toast"
          data-level={t().level}
          onClick={() => dismissToast()}
        >
          {t().text}
        </div>
      )}
    </Show>
  );
};
