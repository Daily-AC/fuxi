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

/** Bottom tab bar 当前 tab：v1-session17 task #9 起 4 tab：
 *  0=玄女 / 1=任务 / 2=通知 / 3=更多。
 *  「项目」「交付」「节点」等全部进 tab 3「更多」hub 二级。 */
export type TabIndex = 0 | 1 | 2 | 3;

/** 「更多」hub 内的二级 sub-page。null = hub 首页（tile grid）。
 *  - nodes / projects / workers / deliverables：原一级 tab 沉到 hub 内
 *  - memory / roles / cron / settings：v1-session17 task #9 新加
 *  workers 复用 v2 残留的 worker 入口（per-agent 私聊）；正常使用从 NodesPage
 *  card tap 进，hub 二级直达入口仅留作 future。 */
export type MoreSubRoute =
  | "nodes"
  | "projects"
  | "workers"
  | "deliverables"
  | "memory"
  | "roles"
  | "cron"
  | "settings"
  | null;

/** NavigationStack 顶部的 push 路由——L2 详情页推 base 之上。
 *  - 任务 tab (1) · kind="task"|"worker"
 *  - 更多 → 项目 (3 + sub="projects") · kind="project"
 *  - 更多 → 交付 (3 + sub="deliverables") · kind="deliverable"
 *  null = 没 push，base 直接见底（base 在更多 hub 下 = 当前 sub-page）。 */
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

  /** 当前 tab。*/
  activeTab: Accessor<TabIndex>;
  setActiveTab(i: TabIndex): void;

  /** 「更多」hub 内的二级 sub-page。仅 activeTab===3 时生效；其他 tab 下 setter 是 noop。*/
  moreSub: Accessor<MoreSubRoute>;
  setMoreSub(s: MoreSubRoute): void;

  /** L2 详情页栈：任务 thread / 项目 detail / 交付 detail。*/
  navRoute: Accessor<NavRoute>;
  navPush(route: NonNullable<NavRoute>): void;
  navPop(): void;
  /** 跨 tab 跳转 helper。
   *
   *  按 route.kind 解析目标：
   *  - task / worker → tab 1（任务）
   *  - project → tab 3（更多） + moreSub="projects"
   *  - deliverable → tab 3（更多） + moreSub="deliverables"
   *  原子化设置，避免调用方手写 setActiveTab + setMoreSub + navPush 多步 race。 */
  navTo(route: NonNullable<NavRoute>): void;

  // ---------- Phase 1 · Topic 一等公民（docs/handoff/v1-session19.md §2-§4） ----------

  /** 当前玄女绑定的 topic_id（UUID 字符串）。初始 null = 未拉到，sidebar 兜底
   *  /api/topics/current 拉一次后填上。switch 完成后由 sidebar 调 `setCurrentTopicId`
   *  把全局态推到最新——XuannvPage 用这个值订阅 reactive，topic 切换时 re-mount
   *  Conversation（重拉历史 + 重连 WS 滤新 topic_id）。 */
  currentTopicId: Accessor<string | null>;
  setCurrentTopicId(id: string): void;

  /** 移动端左滑抽屉开关。桌面端常驻 sidebar 时这个 flag 无效（CSS @media 控）。 */
  sidebarOpen: Accessor<boolean>;
  setSidebarOpen(open: boolean): void;
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
  /** 测试入口：钉死起始 moreSub（默认 null = hub 首页）。*/
  initialMoreSub?: MoreSubRoute;
}

export const ApiProvider: ParentComponent<ApiProviderProps> = (props) => {
  const client = override ?? createHttpClient();
  const [perm, setPerm] = createSignal<NotificationPermission | "unsupported">(
    typeof Notification === "undefined" ? "unsupported" : Notification.permission,
  );
  const [auth, setAuth] = createSignal<AuthState>(props.initialAuth ?? "unknown");
  const [activeTab, _setActiveTab] = createSignal<TabIndex>(props.initialTab ?? 0);
  const [moreSub, _setMoreSub] = createSignal<MoreSubRoute>(props.initialMoreSub ?? null);
  const [navRoute, setNavRoute] = createSignal<NavRoute>(null);
  // Phase 1 · 当前 topic_id 全局态。初始 null 由 sidebar 兜底 /api/topics/current
  // 拉一次后填；switch 完成由调用方主动 setCurrentTopicId（避免 sidebar 跟主对话区
  // 各 fetch 一次出 race）。
  const [currentTopicId, setCurrentTopicId] = createSignal<string | null>(null);
  // 移动端抽屉开关——桌面 CSS @media 把 sidebar 钉死可见，这个 flag 失效。
  const [sidebarOpen, setSidebarOpen] = createSignal<boolean>(false);

  // 哪些 (tab, sub) 组合允许 navPush 二层 detail。
  // 任务 tab 默认开（kind=task / worker）；更多 hub 下仅 projects / deliverables 子页开。
  const navAllowed = (tab: TabIndex, sub: MoreSubRoute): boolean => {
    if (tab === 1) return true; // 任务 tab：kind=task / worker
    if (tab === 3 && (sub === "projects" || sub === "deliverables")) return true;
    return false;
  };

  // tab 切换 / sub 切换时清栈——避免幽灵 top 跨 tab/sub 渲染。
  const setActiveTab = (i: TabIndex): void => {
    if (i !== activeTab()) {
      setNavRoute(null);
      // 离开「更多」tab 时也清 sub，回到 hub 首页比"记住上次 sub-page"更符合
      // 移动端用户预期（再次点 tab 是回首页，二次切回手势只是 tab 切换，不该
      // 跳到上次深处）。
      if (i !== 3) _setMoreSub(null);
    }
    _setActiveTab(i);
  };

  const setMoreSub = (s: MoreSubRoute): void => {
    // 仅在「更多」tab 下生效；其他 tab 下静默 noop（防误操作）。
    if (activeTab() !== 3) return;
    if (s !== moreSub()) setNavRoute(null);
    _setMoreSub(s);
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
        moreSub,
        setMoreSub,
        navRoute,
        // navPush 仅允许 (tab, sub) 组合开启 nav 时；不匹配静默 noop（防 misuse）。
        // 同时按 kind 校验位置匹配——防止 task push 跑到项目子页等错位。
        navPush: (route) => {
          const tab = activeTab();
          const sub = moreSub();
          if (!navAllowed(tab, sub)) return;
          if (route.kind === "task" || route.kind === "worker") {
            if (tab !== 1) return;
          } else if (route.kind === "project") {
            if (tab !== 3 || sub !== "projects") return;
          } else if (route.kind === "deliverable") {
            if (tab !== 3 || sub !== "deliverables") return;
          }
          setNavRoute(route);
        },
        navPop: () => setNavRoute(null),
        currentTopicId,
        setCurrentTopicId,
        sidebarOpen,
        setSidebarOpen,
        // 跨 tab 跳转——绕过 setActiveTab / setMoreSub 的清栈逻辑，直接钉
        // 内部 _setActiveTab + _setMoreSub + setNavRoute 一次到位避免 race。
        navTo: (route) => {
          if (route.kind === "task" || route.kind === "worker") {
            _setActiveTab(1);
            _setMoreSub(null);
          } else if (route.kind === "project") {
            _setActiveTab(3);
            _setMoreSub("projects");
          } else if (route.kind === "deliverable") {
            _setActiveTab(3);
            _setMoreSub("deliverables");
          }
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
