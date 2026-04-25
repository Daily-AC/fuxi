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

export interface ApiContextValue {
  client: ApiClient;
  pushPermission: Accessor<NotificationPermission | "unsupported">;
  enablePush(): Promise<void>;
  authState: Accessor<AuthState>;
  /** 标记已登入（LoginView 成功回调用）。*/
  markLoggedIn(): void;
  /** 标记登出（401 自动触发 / 未来"切换设备"用）。*/
  markLoggedOut(): void;
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
}

export const ApiProvider: ParentComponent<ApiProviderProps> = (props) => {
  const client = override ?? createHttpClient();
  const [perm, setPerm] = createSignal<NotificationPermission | "unsupported">(
    typeof Notification === "undefined" ? "unsupported" : Notification.permission,
  );
  const [auth, setAuth] = createSignal<AuthState>(props.initialAuth ?? "unknown");

  // 探测登入态：试拉一次 fetchTasks（开销小、走 cookie middleware）。
  // 401 → 未登入；其他错误（503 / 网络）→ 视为未登入但保留 LoginView 一次重试机会。
  // β 落地后可以换成 GET /api/auth/me 更直白；目前免新增端点。
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
