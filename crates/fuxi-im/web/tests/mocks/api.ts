// 测试用 mock api client。后端未上时由 tests 注入；e2e 也复用。
import { ApiError, type ApiClient, type LoginRequest, type PairRequest } from "~/lib/api";
import type {
  DispatchRequest,
  DispatchResponse,
  EventHistoryResponse,
  ServerEvent,
  InterveneRequest,
  PushSubscribeRequest,
  TaskListResponse,
  VapidPubResponse,
} from "~/types/events";
import type {
  ConversationHistoryResponse,
  DeliverablesResponse,
  EphemeralResponse,
  InterveneRequestV2,
  NodesResponse,
  ProjectsResponse,
  SandboxView,
  StoredMessage,
  TasksOverview,
  Upload,
} from "~/types/api";

/** mock 鉴权脚本：每次调用消费一个 status，耗尽后回最后一个。
 *  用法：`createMockApi({ auth: { loginSeq: [401, 200] } })` 模拟"先错再对"。*/
export interface AuthScript {
  loginSeq?: number[];
  pairSeq?: number[];
  loginCalls: LoginRequest[];
  pairCalls: PairRequest[];
}

export interface MockState {
  tasks: TaskListResponse["tasks"];
  events: Record<string, ServerEvent[]>;
  intervenes: Array<InterveneRequest | InterveneRequestV2>;
  /** 控制 intervene 调用 status：每次消费一个，耗尽走最后一个。空 = 默认 200。*/
  interveneSeq?: number[];
  dispatches: DispatchRequest[];
  pushed: PushSubscribeRequest[];
  vapid: VapidPubResponse;
  auth: AuthScript;
  /** 历史 conversation messages（按 conv_id 分组）。*/
  history?: Record<string, StoredMessage[]>;
  /** 控制 uploadFile 行为：fail=true 时抛 500，否则按 next id 序列返回。*/
  uploadFail?: boolean;
  uploads: Upload[];
  /** 阶段 4 任务 sheet · /api/tasks 返回值。空时跑空状态。*/
  tasksOverview?: TasksOverview;
  /** v3 #58 dist topology · /api/nodes 返回值。空 = { nodes: [] }。*/
  nodes?: NodesResponse;
  /** Decision 21 phase 1 · /api/projects 返回值。空 = { projects: [] }。*/
  projects?: ProjectsResponse;
  /** Decision 22 phase 1 · /api/deliverables 返回值。空 = { deliverables: [] }。*/
  deliverables?: DeliverablesResponse;
  /** Decision 21 phase 1 · /api/projects/{id}/sandboxes 数据按 project_id 索引。 */
  sandboxesByProject?: Record<string, SandboxView[]>;
  /** Decision 21 phase 3 · /api/projects/{id}/ephemeral 数据按 project_id 索引。 */
  ephemeralByProject?: Record<string, EphemeralResponse>;
}

export interface MockSocket extends Pick<WebSocket, "readyState" | "close"> {
  push(ev: ServerEvent): void;
  addEventListener(type: string, fn: (e: any) => void): void;
  removeEventListener(type: string, fn: (e: any) => void): void;
  dispatchEvent(e: Event): boolean;
}

class FakeSocket implements MockSocket {
  public readyState: number = 0;
  private listeners = new Map<string, Array<(e: any) => void>>();
  static OPEN = 1;
  static CLOSED = 3;

  open(): void {
    this.readyState = FakeSocket.OPEN;
    this.fire("open", new Event("open"));
  }

  push(ev: ServerEvent): void {
    const e = new MessageEvent("message", { data: JSON.stringify(ev) });
    this.fire("message", e);
  }

  close(): void {
    this.readyState = FakeSocket.CLOSED;
    this.fire("close", new CloseEvent("close"));
  }

  addEventListener(type: string, fn: (e: any) => void): void {
    const arr = this.listeners.get(type) ?? [];
    arr.push(fn);
    this.listeners.set(type, arr);
  }

  removeEventListener(type: string, fn: (e: any) => void): void {
    const arr = this.listeners.get(type);
    if (!arr) return;
    this.listeners.set(
      type,
      arr.filter((x) => x !== fn),
    );
  }

  dispatchEvent(e: Event): boolean {
    this.fire(e.type, e);
    return true;
  }

  private fire(type: string, e: any): void {
    for (const fn of this.listeners.get(type) ?? []) fn(e);
  }
}

export interface MockApi extends ApiClient {
  state: MockState;
  /** 主动推一条事件给最近打开的 conv socket */
  pushConv(ev: ServerEvent): void;
  /** 主动推一条事件给指定 task socket */
  pushTask(taskId: string, ev: ServerEvent): void;
  /** 主动推一条事件给指定 worker socket（#N3 / #30 私聊页用） */
  pushWorker(agentId: string, ev: ServerEvent): void;
  /** 主动推一条事件给最近打开的 nodes-stream socket（NodesPage 实时刷用） */
  pushNodesStream(ev: ServerEvent): void;
}

export function createMockApi(initial?: Partial<MockState>): MockApi {
  const state: MockState = {
    tasks: initial?.tasks ?? [],
    events: initial?.events ?? {},
    intervenes: [],
    dispatches: [],
    pushed: [],
    vapid: initial?.vapid ?? {
      public_key:
        "BNNxbcQH3kzTo-Vk1-lF1dQ6r-eJlV6hZcHkBsM5oEpZ0jw0pOe9F1m9w0r5T2wL6kEx4bVa1hKqFjQ8w7yA8kE",
    },
    auth: {
      loginSeq: initial?.auth?.loginSeq,
      pairSeq: initial?.auth?.pairSeq,
      loginCalls: [],
      pairCalls: [],
    },
    interveneSeq: initial?.interveneSeq,
    history: initial?.history,
    uploadFail: initial?.uploadFail,
    uploads: [],
    tasksOverview: initial?.tasksOverview,
    nodes: initial?.nodes,
    projects: initial?.projects,
    deliverables: initial?.deliverables,
    sandboxesByProject: initial?.sandboxesByProject,
    ephemeralByProject: initial?.ephemeralByProject,
  };

  const nextStatus = (seq: number[] | undefined, fallback: number): number => {
    if (!seq || seq.length === 0) return fallback;
    return seq.length === 1 ? (seq[0] ?? fallback) : (seq.shift() ?? fallback);
  };

  let convSocket: FakeSocket | null = null;
  const taskSockets = new Map<string, FakeSocket>();
  const workerSockets = new Map<string, FakeSocket>();
  let nodesStreamSocket: FakeSocket | null = null;

  return {
    state,
    pushConv(ev) {
      convSocket?.push(ev);
    },
    pushTask(taskId, ev) {
      taskSockets.get(taskId)?.push(ev);
    },
    pushWorker(agentId, ev) {
      workerSockets.get(agentId)?.push(ev);
    },
    pushNodesStream(ev) {
      nodesStreamSocket?.push(ev);
    },
    fetchTasks: async () => ({ tasks: state.tasks }),
    fetchTaskEvents: async (taskId): Promise<EventHistoryResponse> => ({
      events: state.events[taskId] ?? [],
      next_cursor: null,
    }),
    intervene: async (req: InterveneRequest | InterveneRequestV2) => {
      state.intervenes.push(req);
      const status = nextStatus(state.interveneSeq, 200);
      if (status !== 200) throw new ApiError(status, statusMessage(status));
      return { ok: true };
    },
    dispatch: async (req: DispatchRequest): Promise<DispatchResponse> => {
      state.dispatches.push(req);
      return { task_id: `t-${state.dispatches.length}` };
    },
    vapidPub: async () => state.vapid,
    pushSubscribe: async (sub) => {
      state.pushed.push(sub);
      return { ok: true };
    },
    login: async (req: LoginRequest) => {
      state.auth.loginCalls.push(req);
      const status = nextStatus(state.auth.loginSeq, 200);
      if (status !== 200) throw new ApiError(status, statusMessage(status));
      return { device_id: `dev-${state.auth.loginCalls.length}` };
    },
    pair: async (req: PairRequest) => {
      state.auth.pairCalls.push(req);
      const status = nextStatus(state.auth.pairSeq, 200);
      if (status !== 200) throw new ApiError(status, statusMessage(status));
      return { device_id: `dev-pair-${state.auth.pairCalls.length}` };
    },
    fetchHistory: async (convId: string): Promise<ConversationHistoryResponse> => {
      const list = state.history?.[convId] ?? [];
      return { messages: list, next_before: null };
    },
    fetchTasksOverview: async (): Promise<TasksOverview> =>
      state.tasksOverview ?? { running: [], completed: [] },
    fetchNodes: async (): Promise<NodesResponse> => state.nodes ?? { nodes: [] },
    fetchProjects: async (): Promise<ProjectsResponse> =>
      state.projects ?? { projects: [] },
    addProject: async (req) => {
      const list = state.projects?.projects ?? [];
      // 简单 mock 行为：撞 id → 409；否则附 view 返。realbacked 走 canonical
      // path 校验 git repo——mock 不验，假定调用方按合法 path 传入。
      const id = req.name ?? "unnamed";
      if (list.some((p) => p.id === id)) {
        throw new ApiError(409, "conflict");
      }
      const view = {
        id,
        canonical_path: req.canonical_path,
        default_branch: req.default_branch ?? "main",
        created_at: new Date().toISOString(),
      };
      state.projects = { projects: [...list, view] };
      return view;
    },
    removeProject: async (id) => {
      const list = state.projects?.projects ?? [];
      const next = list.filter((p) => p.id !== id);
      if (next.length === list.length) {
        throw new ApiError(404, "not found");
      }
      state.projects = { projects: next };
    },
    fetchSandboxes: async (projectId) => {
      // mock：从 state.sandboxesByProject 取（默认空）
      const map = state.sandboxesByProject ?? {};
      return { sandboxes: map[projectId] ?? [] };
    },
    fetchEphemeral: async (projectId) => {
      const map = state.ephemeralByProject ?? {};
      return map[projectId] ?? { active: [], archived: [] };
    },
    fetchDeliverables: async (): Promise<DeliverablesResponse> =>
      state.deliverables ?? { deliverables: [] },
    deliverableFileUrl: (project: string, task: string, name: string): string => {
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      return `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/files/${encodeURIComponent(name)}`;
    },
    deliverableFilePreviewUrl: (project: string, task: string, name: string): string => {
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      return `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/preview/${encodeURIComponent(name)}`;
    },
    acceptDeliverable: async (_project, task) => {
      // mock：把对应 task 的 entries status 都翻 "accepted"
      const list = state.deliverables?.deliverables ?? [];
      const next = list.map((e) =>
        e.task === task ? { ...e, status: "accepted" as const } : e,
      );
      state.deliverables = { deliverables: next };
    },
    rejectDeliverable: async (_project, task) => {
      const list = state.deliverables?.deliverables ?? [];
      const next = list.map((e) =>
        e.task === task ? { ...e, status: "rejected" as const } : e,
      );
      state.deliverables = { deliverables: next };
    },
    fetchWorkerEvents: async (agentId: string): Promise<EventHistoryResponse> => ({
      events: state.events[`worker:${agentId}`] ?? [],
      next_cursor: null,
    }),
    uploadFile: async (
      file: File,
      onProgress?: (r: number) => void,
      signal?: AbortSignal,
    ): Promise<Upload> => {
      if (signal?.aborted) throw new ApiError(0, "aborted");
      if (state.uploadFail) throw new ApiError(500, "upload failed");
      onProgress?.(0.5);
      // 让 abort 在 50% 进度处有机会触发（v3 #50 测 abort 用）
      if (signal?.aborted) throw new ApiError(0, "aborted");
      onProgress?.(1);
      const up: Upload = {
        id: `up-${state.uploads.length + 1}`,
        name: file.name,
        mime: file.type || "application/octet-stream",
        bytes: file.size,
        sha256: `sha-${state.uploads.length + 1}`,
      };
      state.uploads.push(up);
      return up;
    },
    openConvSocket: () => {
      const s = new FakeSocket();
      convSocket = s;
      queueMicrotask(() => s.open());
      return s as unknown as WebSocket;
    },
    openTaskSocket: (taskId) => {
      const s = new FakeSocket();
      taskSockets.set(taskId, s);
      queueMicrotask(() => s.open());
      return s as unknown as WebSocket;
    },
    openWorkerSocket: (agentId) => {
      const s = new FakeSocket();
      workerSockets.set(agentId, s);
      queueMicrotask(() => s.open());
      return s as unknown as WebSocket;
    },
    openNodesStreamSocket: () => {
      const s = new FakeSocket();
      nodesStreamSocket = s;
      queueMicrotask(() => s.open());
      return s as unknown as WebSocket;
    },
    fetchNotifications: async () => ({ notifications: [], unread_count: 0 }),
    markNotificationRead: async () => ({ ok: true as const }),
    closeNotification: async () => ({ ok: true as const }),
    readAllNotifications: async () => ({ ok: true as const, updated: 0 }),
  };
}

function statusMessage(s: number): string {
  if (s === 401) return "unauthorized";
  if (s === 403) return "forbidden";
  if (s === 429) return "too many attempts";
  if (s === 503) return "service unavailable";
  return `http ${s}`;
}
