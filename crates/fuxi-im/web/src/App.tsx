import { Show, onMount, type JSX, type ParentComponent } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { TopBar } from "./components/TopBar";
import { TalkToXuannvBar } from "./components/TalkToXuannvBar";
import { BottomNav } from "./components/BottomNav";
import { ApiProvider, useApi } from "./components/ApiProvider";
import { LoginView } from "./components/LoginView";
import styles from "./App.module.css";

// App shell：顶栏 + 永久"跟玄女说"输入条 + 视图槽 + 底部导航。
// 决策 14 §A：所有 view 顶部固定 intervene 输入条。
// 鉴权 gate（task #10）：未登入显 LoginView；探测中显空底色（防闪烁）。
export const App: ParentComponent = (props): JSX.Element => {
  onMount(() => {
    if ("serviceWorker" in navigator && import.meta.env.PROD) {
      // sw 由 vite-plugin-pwa 自动注入。这里只做兜底。
      navigator.serviceWorker.register("/sw.js").catch(() => undefined);
    }
  });

  return (
    <ApiProvider>
      <AuthGate>{props.children}</AuthGate>
    </ApiProvider>
  );
};

const AuthGate: ParentComponent = (props) => {
  const { authState, markLoggedIn } = useApi();
  const navigate = useNavigate();

  return (
    <>
      <Show when={authState() === "in"}>
        <div class={styles.shell}>
          <TopBar />
          <TalkToXuannvBar />
          <main class={styles.main} data-testid="view-slot">
            {props.children}
          </main>
          <BottomNav />
        </div>
      </Show>
      <Show when={authState() === "out"}>
        <LoginView
          onSuccess={() => {
            markLoggedIn();
            navigate("/", { replace: true });
          }}
        />
      </Show>
      <Show when={authState() === "unknown"}>
        <div class={styles.probing} data-testid="auth-probing" aria-hidden="true" />
      </Show>
    </>
  );
};
