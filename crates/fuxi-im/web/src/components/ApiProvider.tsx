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
 *  v3 仅在"任务 tab"下生效（Layer 1 任务列表 → Layer 2 任务 thread）。
 *  null = 没 push，base 直接见底。
 *
 *  类型说明：
 *    - kind: "task" · v3 主路：任务卡片 → 任务 thread (#38/#N3' 落)
 *    - kind: "worker" · v2 残留 · per-worker 私聊（#40/#N5' 推 v3 时移除）
 *      保留只是为了让 TasksPage v2 member-row tap 不立即编译错；
 *      App.tsx renderTaskTop 仅渲染 kind==="task"，其他 kind 视作 noop。*/
export type NavRoute =
  | { kind: "task"; task_id: string; title?: string }
  | { kind: "worker"; agent_id: string; role_display?: string }
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

  // tab 切换时清空 navRoute · 跨 tab 不保留二层 push（spec §A "navPush 仅任务 tab 下生效"）
  const setActiveTab = (i: TabIndex): void => {
    if (i !== 1) setNavRoute(null);
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
        // navPush 只允许在任务 tab 下；其他 tab 调用静默 noop（防御 misuse）
        navPush: (route) => {
          if (activeTab() !== 1) return;
          setNavRoute(route);
        },
        navPop: () => setNavRoute(null),
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
