// 测试用 mock api client。后端未上时由 tests 注入；e2e 也复用。
import type { ApiClient } from "~/lib/api";
import type {
  DispatchRequest,
  DispatchResponse,
  EventHistoryResponse,
  EventKind,
  InterveneRequest,
  PushSubscribeRequest,
  TaskListResponse,
  VapidPubResponse,
} from "~/types/events";

export interface MockState {
  tasks: TaskListResponse["tasks"];
  events: Record<string, EventKind[]>;
  intervenes: InterveneRequest[];
  dispatches: DispatchRequest[];
  pushed: PushSubscribeRequest[];
  vapid: VapidPubResponse;
}

export interface MockSocket extends Pick<WebSocket, "readyState" | "close"> {
  push(ev: EventKind): void;
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

  push(ev: EventKind): void {
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
  pushConv(ev: EventKind): void;
  /** 主动推一条事件给指定 task socket */
  pushTask(taskId: string, ev: EventKind): void;
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
  };

  let convSocket: FakeSocket | null = null;
  const taskSockets = new Map<string, FakeSocket>();

  return {
    state,
    pushConv(ev) {
      convSocket?.push(ev);
    },
    pushTask(taskId, ev) {
      taskSockets.get(taskId)?.push(ev);
    },
    fetchTasks: async () => ({ tasks: state.tasks }),
    fetchTaskEvents: async (taskId): Promise<EventHistoryResponse> => ({
      events: state.events[taskId] ?? [],
      next_cursor: null,
    }),
    intervene: async (req: InterveneRequest) => {
      state.intervenes.push(req);
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
  };
}
