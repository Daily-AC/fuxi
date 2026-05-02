import {
  Show,
  Switch,
  Match,
  onMount,
  type Component,
  type JSX,
} from "solid-js";
import { ApiProvider, useApi, type TabIndex } from "./components/ApiProvider";
import { LoginView } from "./components/LoginView";
import { BottomTabBar, type TabSpec } from "./components/BottomTabBar";
import { NavigationStack } from "./components/NavigationStack";
import { Toast } from "./components/Toast";
import { DeliverableDetailPage } from "./views/pages/DeliverableDetailPage";
import { DeliverablesPage } from "./views/pages/DeliverablesPage";
import { NodesPage } from "./views/pages/NodesPage";
import { ProjectDetailPage } from "./views/pages/ProjectDetailPage";
import { ProjectsPage } from "./views/pages/ProjectsPage";
import { XuannvPage } from "./views/pages/XuannvPage";
import { TasksPage } from "./views/pages/TasksPage";
import { TaskThreadPage } from "./views/pages/TaskThreadPage";
import styles from "./App.module.css";

// 顶层 shell · v3 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §A
//   未登入 → LoginView
//   登入 → MainShell{ activeTab content + BottomTabBar }
//
// v3 心智 (#36/#N1')：
//   - tab bar 替代 horizontal pager（不允许 tab 间手势切换）
//   - NavigationStack 仅在"任务 tab"下生效（Layer 1 任务列表 → Layer 2 任务 thread）
//   - 玄女/节点 tab 各自一页，没有二层导航
//
// 任务 tab Layer 2 thread (#39/#N4') 之前用 navRoute placeholder 占位，
// 走 navPush({ kind: "task", task_id }) 触发；本 task 仅 wire 闭环，placeholder 内容
// 给到 #39 时再渲染 TaskThread 组件。
export const App: Component = (): JSX.Element => {
  onMount(() => {
    if ("serviceWorker" in navigator && import.meta.env.PROD) {
      navigator.serviceWorker.register("/sw.js").catch(() => undefined);
    }
  });

  return (
    <ApiProvider>
      <AuthGate />
      <Toast />
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

const TABS: TabSpec[] = [
  { key: "xuannv", label: "玄女" },
  { key: "tasks", label: "任务" },
  { key: "projects", label: "项目" },
  { key: "deliverables", label: "交付" },
  { key: "nodes", label: "节点" },
];

const MainShell: Component = () => {
  const { activeTab, setActiveTab, navRoute, navPop } = useApi();

  // 任务 tab Layer 2 · v3 真 TaskThreadPage（#39 已落）
  // kind === "worker"（v2 残留）保留兜底 stub，正常 v3 流不应触发
  const renderTaskTop = (): JSX.Element | undefined => {
    const r = navRoute();
    if (!r) return undefined;
    if (r.kind === "task") {
      return <TaskThreadPage task_id={r.task_id} fallback_title={r.title} />;
    }
    if (r.kind === "worker") {
      return (
        <div class={styles.taskTopStub} data-testid="worker-route-legacy">
          <header class={styles.stubHead}>
            <button
              type="button"
              class={styles.stubBack}
              onClick={() => navPop()}
              data-testid="task-thread-back"
              aria-label="返回"
            >
              ‹ 返回
            </button>
            <span class={styles.stubTitle}>{r.role_display ?? "门客"}</span>
            <span class={styles.stubRight} aria-hidden="true" />
          </header>
          <div class={styles.stubBody}>
            <p class={styles.stubMsg}>
              v2 私聊页路由已废，请从任务卡 push 进任务 thread。
            </p>
          </div>
        </div>
      );
    }
    // tab 1 不渲染 project / deliverable kind（路由保护已在 ApiProvider 拦），
    // 走到这里说明被错误推入——返 undefined 不渲染 top 即可。
    return undefined;
  };

  // 项目 tab Layer 2 · Decision 21 phase 3 ProjectDetailPage
  const renderProjectTop = (): JSX.Element | undefined => {
    const r = navRoute();
    if (!r || r.kind !== "project") return undefined;
    return <ProjectDetailPage project_id={r.project_id} />;
  };

  // 交付 tab Layer 2 · Decision 22 phase 3 DeliverableDetailPage
  const renderDeliverableTop = (): JSX.Element | undefined => {
    const r = navRoute();
    if (!r || r.kind !== "deliverable") return undefined;
    return (
      <DeliverableDetailPage
        project_id={r.project_id}
        task_id={r.task_id}
      />
    );
  };

  return (
    <div class={styles.shell} data-testid="main-shell">
      <main class={styles.main} data-testid="tab-content">
        <Switch>
          <Match when={activeTab() === 0}>
            <XuannvPage />
          </Match>
          <Match when={activeTab() === 1}>
            <NavigationStack
              base={<TasksPage />}
              top={renderTaskTop()}
              onPop={navPop}
            />
          </Match>
          <Match when={activeTab() === 2}>
            <NavigationStack
              base={<ProjectsPage />}
              top={renderProjectTop()}
              onPop={navPop}
            />
          </Match>
          <Match when={activeTab() === 3}>
            <NavigationStack
              base={<DeliverablesPage />}
              top={renderDeliverableTop()}
              onPop={navPop}
            />
          </Match>
          <Match when={activeTab() === 4}>
            <NodesPage />
          </Match>
        </Switch>
      </main>
      <BottomTabBar
        tabs={TABS}
        active={activeTab()}
        onChange={(i: TabIndex) => setActiveTab(i)}
      />
    </div>
  );
};
