import {
  createContext,
  useContext,
  type Accessor,
  type ParentComponent,
  createSignal,
  onMount,
} from "solid-js";
import { ApiError, createHttpClient, type ApiClient } from "~/lib/api";
import { ensurePushSubscription } from "~/lib/push";

/** 登入态：unknown = 还在探测；in = cookie 有效；out = 未登入或 cookie 失效。*/
export type AuthState = "unknown" | "in" | "out";

/** Bottom tab bar 当前 tab：0=玄女 / 1=任务 / 2=项目 / 3=交付 / 4=节点。
 *  设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §A
 *  v2 的 PageIndex 已淘汰（horizontal pager 路线被 supersede）。
 *  Decision 21/22 phase 1 加 项目 / 交付 两个 tab。*/
export type TabIndex = 0 | 1 | 2 | 3 | 4;

/** NavigationStack 顶部的 push 路由。
 *  - 任务 tab (1) · kind="task"|"worker"
 *  - 项目 tab (2) · kind="project" · Decision 21 phase 3 项目详情页
 *  - 交付 tab (3) · kind="deliverable" · Decision 22 phase 3 交付详情页
 *  null = 没 push，base 直接见底。
 *
 *  类型说明：
 *    - kind: "task" · v3 主路：任务卡片 → 任务 thread (#38/#N3' 落)
 *    - kind: "worker" · v2 残留 · per-worker 私聊（#40/#N5' 推 v3 时移除）
 *      保留只是为了让 TasksPage v2 member-row tap 不立即编译错；
 *      App.tsx renderTaskTop 仅渲染 kind==="task"，其他 kind 视作 noop。
 *    - kind: "project" · 项目 detail 页：sandboxes / L2 active+archive / 交付汇总
 *    - kind: "deliverable" · 交付 detail 页：manifest 全文 + 文件预览 + accept/reject 入口 */
export type NavRoute =
  | { kind: "task"; task_id: string; title?: string }
  | { kind: "worker"; agent_id: string; role_display?: string }
  | { kind: "project"; project_id: string }
  | { kind: "deliverable"; project_id: string; task_id: string }
  | null;

export interface ApiContextValue {
  client: ApiClient;
  pushPermission: Accessor<NotificationPermission | "unsupported">;
  enablePush(): Promise<void>;
  authState: Accessor<AuthState>;
  /** 标记已登入（LoginView 成功回调用）。*/
  markLoggedIn(): void;
  /** 标记登出（401 自动触发 / 未来"切换设备"用）。*/
  markLoggedOut(): void;

  /** 当前 tab（0=玄女, 1=任务, 2=节点）。*/
  activeTab: Accessor<TabIndex>;
  setActiveTab(i: TabIndex): void;

  /** 任务 tab 的 NavigationStack 路由。其他 tab 下读到 null。
   *  navPush/navPop 仅在任务 tab 下生效；玄女/节点 tab 下调用是 noop。*/
  navRoute: Accessor<NavRoute>;
  navPush(route: NonNullable<NavRoute>): void;
  navPop(): void;
  /** 跨 tab 跳转 helper · Decision 21/22 phase 3 polish。
   *
   *  按 route.kind 解析目标 tab（task/worker → 1，project → 2，deliverable → 3）
   *  然后 setActiveTab + navPush 一步到位。比调用方手写 setActiveTab(3) + navPush
   *  少一次重渲染，且不会被 setActiveTab 的"切到无 nav tab 时清栈"逻辑误清。*/
  navTo(route: NonNullable<NavRoute>): void;
}

const ApiContext = createContext<ApiContextValue>();

// 测试钩子：让 vitest 注入 mock client。
// production 走真实 fetch；测试通过 setApiOverride 替换。
let override: ApiClient | null = null;
export function setApiOverride(client: ApiClient | null): void {
  override = client;
}

export interface ApiProviderProps {
  /** 测试入口：跳过 onMount 探测，直接钉死 authState。*/
  initialAuth?: AuthState;
  /** 测试入口：钉死起始 tab（默认 0=玄女）。*/
  initialTab?: TabIndex;
}

export const ApiProvider: ParentComponent<ApiProviderProps> = (props) => {
  const client = override ?? createHttpClient();
  const [perm, setPerm] = createSignal<NotificationPermission | "unsupported">(
    typeof Notification === "undefined" ? "unsupported" : Notification.permission,
  );
  const [auth, setAuth] = createSignal<AuthState>(props.initialAuth ?? "unknown");
  const [activeTab, _setActiveTab] = createSignal<TabIndex>(props.initialTab ?? 0);
  const [navRoute, setNavRoute] = createSignal<NavRoute>(null);

  // tab 切换时清空 navRoute · 跨 tab 不保留二层 push（spec §A "navPush 仅任务 tab 下生效"）。
  // Decision 21/22 phase 3：项目 / 交付 tab 也开二层 push（项目详情 / 交付详情）。
  // 切到 0 (玄女) / 4 (节点) 这类无二层的 tab 时强制清栈避免幽灵 top 渲染。
  const TABS_WITH_NAV: ReadonlyArray<TabIndex> = [1, 2, 3];
  const setActiveTab = (i: TabIndex): void => {
    if (!TABS_WITH_NAV.includes(i)) setNavRoute(null);
    _setActiveTab(i);
  };

  // 探测登入态：试拉一次 fetchTasks（开销小、走 cookie middleware）。
  const probe = async (): Promise<void> => {
    if (props.initialAuth) return;
    try {
      await client.fetchTasks(true);
      setAuth("in");
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) setAuth("out");
      else setAuth("out");
    }
  };

  onMount(() => {
    if (typeof Notification !== "undefined") setPerm(Notification.permission);
    void probe();
  });

  const enablePush = async (): Promise<void> => {
    try {
      await ensurePushSubscription(client);
      if (typeof Notification !== "undefined") setPerm(Notification.permission);
    } catch (err) {
      console.warn("push subscribe failed", err);
    }
  };

  return (
    <ApiContext.Provider
      value={{
        client,
        pushPermission: perm,
        enablePush,
        authState: auth,
        markLoggedIn: () => setAuth("in"),
        markLoggedOut: () => setAuth("out"),
        activeTab,
        setActiveTab,
        navRoute,
        // navPush 仅允许在有二层栈的 tab 下；其他 tab 调用静默 noop（防御 misuse）。
        // 同时按 kind 校验 tab 匹配 —— 防止 task push 跑到项目 tab 上让 App.Switch 渲染错位。
        navPush: (route) => {
          const tab = activeTab();
          if (!TABS_WITH_NAV.includes(tab)) return;
          if ((route.kind === "task" || route.kind === "worker") && tab !== 1) return;
          if (route.kind === "project" && tab !== 2) return;
          if (route.kind === "deliverable" && tab !== 3) return;
          setNavRoute(route);
        },
        navPop: () => setNavRoute(null),
        // 跨 tab 跳转 · setActiveTab + navPush 原子化。绕过 setActiveTab 的清栈
        // 逻辑（直接走内部 _setActiveTab）避免 race。
        navTo: (route) => {
          const targetTab: TabIndex =
            route.kind === "task" || route.kind === "worker"
              ? 1
              : route.kind === "project"
                ? 2
                : 3;
          _setActiveTab(targetTab);
          setNavRoute(route);
        },
      }}
    >
      {props.children}
    </ApiContext.Provider>
  );
};

export function useApi(): ApiContextValue {
  const v = useContext(ApiContext);
  if (!v) throw new Error("useApi outside ApiProvider");
  return v;
}
