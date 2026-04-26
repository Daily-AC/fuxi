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

/** Pager 当前页索引：0=节点 / 1=玄女主对话 / 2=任务树。
 *  设计 spec: docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §B */
export type PageIndex = 0 | 1 | 2;

/** NavigationStack 顶部的 push 路由（v1 只有"私聊门客"一种）。
 *  null = 没 push，pager 直接见底。*/
export type NavRoute = { kind: "worker"; agent_id: string; role_display?: string } | null;

export interface ApiContextValue {
  client: ApiClient;
  pushPermission: Accessor<NotificationPermission | "unsupported">;
  enablePush(): Promise<void>;
  authState: Accessor<AuthState>;
  /** 标记已登入（LoginView 成功回调用）。*/
  markLoggedIn(): void;
  /** 标记登出（401 自动触发 / 未来"切换设备"用）。*/
  markLoggedOut(): void;

  /** 当前 pager 页（0=节点, 1=玄女, 2=任务树）。*/
  currentPage: Accessor<PageIndex>;
  setCurrentPage(p: PageIndex): void;

  /** NavigationStack 当前路由（v1 只支持 0 或 1 层）。null = 没 push。*/
  navRoute: Accessor<NavRoute>;
  /** 推一个新路由（覆盖式 v1，下个 task #N3 才会实际渲染私聊页）。*/
  navPush(route: NonNullable<NavRoute>): void;
  /** 弹回 pager。*/
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
  /** 测试入口：钉死起始 page（默认 1=玄女）。*/
  initialPage?: PageIndex;
}

export const ApiProvider: ParentComponent<ApiProviderProps> = (props) => {
  const client = override ?? createHttpClient();
  const [perm, setPerm] = createSignal<NotificationPermission | "unsupported">(
    typeof Notification === "undefined" ? "unsupported" : Notification.permission,
  );
  const [auth, setAuth] = createSignal<AuthState>(props.initialAuth ?? "unknown");
  const [currentPage, setCurrentPage] = createSignal<PageIndex>(props.initialPage ?? 1);
  const [navRoute, setNavRoute] = createSignal<NavRoute>(null);

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
        currentPage,
        setCurrentPage,
        navRoute,
        navPush: (route) => setNavRoute(route),
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
