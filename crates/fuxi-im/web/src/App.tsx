import type { JSX, ParentComponent } from "solid-js";
import { onMount } from "solid-js";
import { TopBar } from "./components/TopBar";
import { TalkToXuannvBar } from "./components/TalkToXuannvBar";
import { BottomNav } from "./components/BottomNav";
import { ApiProvider } from "./components/ApiProvider";
import styles from "./App.module.css";

// App shell：顶栏 + 永久"跟玄女说"输入条 + 视图槽 + 底部导航。
// 决策 14 §A：所有 view 顶部固定 intervene 输入条。
export const App: ParentComponent = (props): JSX.Element => {
  onMount(() => {
    if ("serviceWorker" in navigator && import.meta.env.PROD) {
      // sw 由 vite-plugin-pwa 自动注入。这里只做兜底。
      navigator.serviceWorker.register("/sw.js").catch(() => undefined);
    }
  });

  return (
    <ApiProvider>
      <div class={styles.shell}>
        <TopBar />
        <TalkToXuannvBar />
        <main class={styles.main} data-testid="view-slot">
          {props.children}
        </main>
        <BottomNav />
      </div>
    </ApiProvider>
  );
};
