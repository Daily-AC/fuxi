import {
  Show,
  onMount,
  type Component,
  type JSX,
} from "solid-js";
import { ApiProvider, useApi, type PageIndex } from "./components/ApiProvider";
import { LoginView } from "./components/LoginView";
import { Pager } from "./components/Pager";
import { NavigationStack } from "./components/NavigationStack";
import { NodesPage } from "./views/pages/NodesPage";
import { XuannvPage } from "./views/pages/XuannvPage";
import { TasksPage } from "./views/pages/TasksPage";
import styles from "./App.module.css";

// 顶层 shell · 设计 spec: docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §B
//   未登入 → LoginView
//   登入 → NavigationStack(base=Pager[节点·玄女·任务树], top=私聊页)
// 私聊页 v1 留 push API stub，#N3 才真渲染。
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

const PAGE_LABELS = ["节点", "玄女", "任务树"];

const MainShell: Component = () => {
  const { currentPage, setCurrentPage, navRoute, navPop } = useApi();

  // 私聊页 placeholder · #N3 才真接 WS / intervene。给 NavigationStack 喂占位，
  // 包含 "‹ 玄女" 返回按钮触发 navPop，让 shell 闭环可测。
  const renderTop = (): JSX.Element | undefined => {
    const r = navRoute();
    if (!r) return undefined;
    return (
      <div class={styles.workerStub} data-testid="worker-stub">
        <header class={styles.workerHead}>
          <button
            type="button"
            class={styles.backBtn}
            onClick={() => navPop()}
            data-testid="worker-back"
            aria-label="返回玄女"
          >
            ‹ 玄女
          </button>
          <span class={styles.workerTitle}>{r.role_display ?? "门客"}</span>
          <span class={styles.workerHeadRight} aria-hidden="true" />
        </header>
        <div class={styles.workerBody}>
          <p class={styles.workerStubMsg}>
            私聊页 placeholder（#N3 实装橙色识别 + 任务 banner + 真消息流）。
          </p>
          <p class={styles.workerStubAgent}>
            <span class="agent-id">{r.agent_id}</span>
          </p>
        </div>
      </div>
    );
  };

  const onIndexChange = (i: number): void => {
    const clamped = Math.max(0, Math.min(2, i)) as PageIndex;
    setCurrentPage(clamped);
  };

  return (
    <div class={styles.shell} data-testid="main-shell">
      <NavigationStack
        base={
          <Pager
            pages={[<NodesPage />, <XuannvPage />, <TasksPage />]}
            pageLabels={PAGE_LABELS}
            index={currentPage()}
            onIndexChange={onIndexChange}
          />
        }
        top={renderTop()}
        onPop={navPop}
      />
    </div>
  );
};
