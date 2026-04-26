import { createSignal, onMount, type Component, type JSX, Show } from "solid-js";
import { ApiProvider, useApi } from "./components/ApiProvider";
import { LoginView } from "./components/LoginView";
import { Header } from "./components/Header";
import { Composer } from "./components/Composer";
import { Conversation } from "./views/Conversation";
import styles from "./App.module.css";

// 顶层 shell：未登入 → LoginView；登入 → Header + Conversation + Composer。
// 阶段 1：空态视觉骨架；阶段 2 起接消息流。
export const App: Component = (): JSX.Element => {
  onMount(() => {
    if ("serviceWorker" in navigator && import.meta.env.PROD) {
      navigator.serviceWorker.register("/sw.js").catch(() => undefined);
    }
  });

  return (
    <ApiProvider>
      <AuthGate />
    </ApiProvider>
  );
};

const AuthGate: Component = () => {
  const { authState, markLoggedIn } = useApi();
  return (
    <>
      <Show when={authState() === "in"}>
        <MainShell />
      </Show>
      <Show when={authState() === "out"}>
        <LoginView onSuccess={() => markLoggedIn()} />
      </Show>
      <Show when={authState() === "unknown"}>
        <div class={styles.probing} data-testid="auth-probing" aria-hidden="true" />
      </Show>
    </>
  );
};

const MainShell: Component = () => {
  // 阶段 1 stub。signals 留好接下阶段。
  const [messages] = createSignal<unknown[]>([]);
  const [online, _setOnline] = createSignal(false);
  void _setOnline;
  const [_tasksOpen, setTasksOpen] = createSignal(false);
  const [_nodesOpen, setNodesOpen] = createSignal(false);
  void _tasksOpen;
  void _nodesOpen;

  // 阶段 2：换成真 intervene。阶段 1 仅 stub 防 form 报错。
  const handleSubmit = async (_text: string): Promise<void> => {
    void _text;
  };

  return (
    <div class={styles.shell} data-testid="main-shell">
      <Header
        online={online()}
        onOpenTasks={() => setTasksOpen(true)}
        onOpenNodes={() => setNodesOpen(true)}
      />
      <Conversation messages={messages} />
      <Composer onSubmit={handleSubmit} />
    </div>
  );
};
