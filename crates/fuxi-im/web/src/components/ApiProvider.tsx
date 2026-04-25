import {
  createContext,
  useContext,
  type Accessor,
  type ParentComponent,
  createSignal,
  onMount,
} from "solid-js";
import { createHttpClient, type ApiClient } from "~/lib/api";
import { ensurePushSubscription } from "~/lib/push";

export interface ApiContextValue {
  client: ApiClient;
  pushPermission: Accessor<NotificationPermission | "unsupported">;
  enablePush(): Promise<void>;
}

const ApiContext = createContext<ApiContextValue>();

// 测试钩子：让 vitest 注入 mock client。
// production 走真实 fetch；测试通过 setApiOverride 替换。
let override: ApiClient | null = null;
export function setApiOverride(client: ApiClient | null): void {
  override = client;
}

export const ApiProvider: ParentComponent = (props) => {
  const client = override ?? createHttpClient();
  const [perm, setPerm] = createSignal<NotificationPermission | "unsupported">(
    typeof Notification === "undefined" ? "unsupported" : Notification.permission,
  );

  onMount(() => {
    if (typeof Notification !== "undefined") setPerm(Notification.permission);
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
    <ApiContext.Provider value={{ client, pushPermission: perm, enablePush }}>
      {props.children}
    </ApiContext.Provider>
  );
};

export function useApi(): ApiContextValue {
  const v = useContext(ApiContext);
  if (!v) throw new Error("useApi outside ApiProvider");
  return v;
}
