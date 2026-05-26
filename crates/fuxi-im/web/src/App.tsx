import {
  Show,
  Switch,
  Match,
  createMemo,
  createResource,
  onCleanup,
  onMount,
  type Component,
  type JSX,
} from "solid-js";
import {
  ApiProvider,
  useApi,
  type MoreSubRoute,
  type TabIndex,
} from "./components/ApiProvider";
import { LoginView } from "./components/LoginView";
import { BottomTabBar, type TabSpec } from "./components/BottomTabBar";
import { MoreSubShell } from "./components/MoreSubShell";
import { NavigationStack } from "./components/NavigationStack";
import { Toast } from "./components/Toast";
import { TopicSidebar } from "./components/TopicSidebar";
import { CronPage } from "./views/pages/CronPage";
import { DeliverableDetailPage } from "./views/pages/DeliverableDetailPage";
import { DeliverablesPage } from "./views/pages/DeliverablesPage";
import { MemoryPage } from "./views/pages/MemoryPage";
import { MorePage } from "./views/pages/MorePage";
import { NodesPage } from "./views/pages/NodesPage";
import { NotificationsPage } from "./views/pages/NotificationsPage";
import { ProjectDetailPage } from "./views/pages/ProjectDetailPage";
import { ProjectsPage } from "./views/pages/ProjectsPage";
import { RolesPage } from "./views/pages/RolesPage";
import { SettingsPage } from "./views/pages/SettingsPage";
import { XuannvPage } from "./views/pages/XuannvPage";
import { TasksPage } from "./views/pages/TasksPage";
import { TaskThreadPage } from "./views/pages/TaskThreadPage";
import styles from "./App.module.css";

// 顶层 shell · v1-session17 task #9 4 tab + 「更多」hub
//   未登入 → LoginView
//   登入 → MainShell{ activeTab content + BottomTabBar }
//
// tab 模型（docs/handoff/v1-session16.md §2.2 方案 A）：
//   0 玄女 / 1 任务 / 2 通知 / 3 更多
//
// tab 3 「更多」分两层：
//   - moreSub=null → MorePage（tile grid hub）
//   - moreSub=<sub> → 对应 sub-page，外面套 MoreSubShell（顶部 ‹ 更多 返回）
//   sub=projects / deliverables 内部还有 L2 detail（NavigationStack push）—
//   navRoute 跟 v3 一样按 kind 渲染对应 detail 页。
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

const BASE_TABS: ReadonlyArray<TabSpec> = [
  { key: "xuannv", label: "玄女" },
  { key: "tasks", label: "任务" },
  { key: "notifications", label: "通知" },
  { key: "more", label: "更多" },
];

const MainShell: Component = () => {
  const { client, activeTab, setActiveTab, moreSub, navRoute, navPop } = useApi();

  // v1-session16 通知红点 · 后台轮询 GET /api/notifications 拿 unread_count。
  // 简单起步不接 WS——15s 间隔够用（bug / handoff offer 是分钟级事件，不抢秒级）。
  const [notif, { refetch: refetchNotif }] = createResource(() =>
    client.fetchNotifications({ limit: 1 }),
  );
  onMount(() => {
    const t = setInterval(() => {
      void refetchNotif();
    }, 15000);
    onCleanup(() => clearInterval(t));
  });
  const tabs = createMemo<TabSpec[]>(() => {
    const unread = notif()?.unread_count ?? 0;
    return BASE_TABS.map((t) =>
      t.key === "notifications" ? { ...t, badge: unread } : t,
    );
  });

  // 任务 tab L2 · v3 真 TaskThreadPage（#39 已落）；worker kind 为 v2 残留兜底 stub。
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
    return undefined;
  };

  // 「更多 → 项目」L2 detail：navRoute kind="project"
  const renderProjectTop = (): JSX.Element | undefined => {
    const r = navRoute();
    if (!r || r.kind !== "project") return undefined;
    return <ProjectDetailPage project_id={r.project_id} />;
  };

  // 「更多 → 交付」L2 detail：navRoute kind="deliverable"
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
      <div class={styles.bodyRow}>
        {/* Phase 1 · 桌面 240px 常驻 sidebar；移动端 fixed drawer 由 sidebar 自管 transform */}
        <TopicSidebar />
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
              <NotificationsPage />
            </Match>
            <Match when={activeTab() === 3}>
              <MoreTabContent
                sub={moreSub()}
                renderProjectTop={renderProjectTop}
                renderDeliverableTop={renderDeliverableTop}
                onPopL2={navPop}
              />
            </Match>
          </Switch>
        </main>
      </div>
      <BottomTabBar
        tabs={tabs()}
        active={activeTab()}
        onChange={(i: TabIndex) => {
          setActiveTab(i);
          // 进入「通知」tab 时立即刷一次 unread_count（NotificationsPage 自己
          // 会调 readAllNotifications 清零，badge 跟着消失）。
          if (i === 2) {
            setTimeout(() => {
              void refetchNotif();
            }, 200);
          }
        }}
      />
    </div>
  );
};

interface MoreTabContentProps {
  sub: MoreSubRoute;
  renderProjectTop: () => JSX.Element | undefined;
  renderDeliverableTop: () => JSX.Element | undefined;
  onPopL2: () => void;
}

// 「更多」tab 内容分发——sub null = MorePage hub；非 null = MoreSubShell 套子页。
// 子页中 projects / deliverables 内部还有 L2 detail（NavigationStack push）。
const MoreTabContent: Component<MoreTabContentProps> = (props) => {
  return (
    <Switch fallback={<MorePage />}>
      <Match when={props.sub === null}>
        <MorePage />
      </Match>
      <Match when={props.sub === "nodes"}>
        <MoreSubShell title="节点" hideTitle testIdSuffix="nodes">
          <NodesPage />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "projects"}>
        <MoreSubShell title="项目" hideTitle testIdSuffix="projects">
          <NavigationStack
            base={<ProjectsPage />}
            top={props.renderProjectTop()}
            onPop={props.onPopL2}
          />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "deliverables"}>
        <MoreSubShell title="交付物" hideTitle testIdSuffix="deliverables">
          <NavigationStack
            base={<DeliverablesPage />}
            top={props.renderDeliverableTop()}
            onPop={props.onPopL2}
          />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "memory"}>
        <MoreSubShell title="记忆" testIdSuffix="memory">
          <MemoryPage />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "roles"}>
        <MoreSubShell title="角色" testIdSuffix="roles">
          <RolesPage />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "cron"}>
        <MoreSubShell title="更漏" testIdSuffix="cron">
          <CronPage />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "settings"}>
        <MoreSubShell title="设置" testIdSuffix="settings">
          <SettingsPage />
        </MoreSubShell>
      </Match>
      <Match when={props.sub === "workers"}>
        <MoreSubShell title="工作者" testIdSuffix="workers">
          <p class={styles.subStub} data-testid="page-workers-stub">
            通过「更多 → 节点」点 worker 行进入门客详情。
          </p>
        </MoreSubShell>
      </Match>
    </Switch>
  );
};
