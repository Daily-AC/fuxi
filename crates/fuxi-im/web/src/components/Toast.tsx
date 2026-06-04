import { Show, createEffect, useContext, type Component } from "solid-js";
import { currentToast, dismissToast } from "~/lib/toast";
import { MascotContext } from "~/components/Mascot/MascotController";
import styles from "./Toast.module.css";

// 全局 toast · App shell 顶层挂一个，组件用 lib/toast::pushToast 触发。
// 单 toast，不堆叠（移动端遮挡 + 用户认知负担）。
export const Toast: Component = () => {
  // Task 32 · error 级 toast → 玄女惊讶。Toast 在 MascotProvider 内（App.tsx），能拿
  // dispatch；lib/toast 是无 context 纯模块够不到。这里 createEffect 监听 currentToast，
  // 每出现一条「新」error toast 派一次 {type:"error"}。用 lastErrorId 去重，避免
  // 同一条 toast 在每次 render 重复派（surprise→settle→surprise 抖动）。
  //
  // 直接 useContext(MascotContext) 而非 useMascot()——后者无 provider 时 throw。
  // Toast 的核心职责是渲染 toast，吉祥物联动是「附加」；脱离 provider 单测/复用
  // 场景下应静默降级而非崩。真实 app 里 provider 永远在。
  const mascot = useContext(MascotContext);
  let lastErrorId = 0;
  createEffect(() => {
    const t = currentToast();
    if (mascot && t && t.level === "error" && t.id !== lastErrorId) {
      lastErrorId = t.id;
      mascot.dispatch({ type: "error" });
    }
  });

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
