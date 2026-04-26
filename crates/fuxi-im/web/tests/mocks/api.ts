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
  InterveneRequestV2,
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
  };

  const nextStatus = (seq: number[] | undefined, fallback: number): number => {
    if (!seq || seq.length === 0) return fallback;
    return seq.length === 1 ? (seq[0] ?? fallback) : (seq.shift() ?? fallback);
  };

  let convSocket: FakeSocket | null = null;
  const taskSockets = new Map<string, FakeSocket>();
  const workerSockets = new Map<string, FakeSocket>();

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
    fetchWorkerEvents: async (agentId: string): Promise<EventHistoryResponse> => ({
      events: state.events[`worker:${agentId}`] ?? [],
      next_cursor: null,
    }),
    uploadFile: async (file: File, onProgress?: (r: number) => void): Promise<Upload> => {
      if (state.uploadFail) throw new ApiError(500, "upload failed");
      onProgress?.(0.5);
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
  };
}

function statusMessage(s: number): string {
  if (s === 401) return "unauthorized";
  if (s === 403) return "forbidden";
  if (s === 429) return "too many attempts";
  if (s === 503) return "service unavailable";
  return `http ${s}`;
}
