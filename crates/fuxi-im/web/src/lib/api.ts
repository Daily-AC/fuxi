// REST + WS 客户端。契约见 decision-14 §C；后端未上时切换 mockFetch。
import type {
  DispatchRequest,
  DispatchResponse,
  EventHistoryResponse,
  InterveneRequest,
  PushSubscribeRequest,
  TaskListResponse,
  VapidPubResponse,
} from "~/types/events";

export interface ApiClient {
  fetchTasks(rootOnly: boolean): Promise<TaskListResponse>;
  fetchTaskEvents(taskId: string, fromCursor?: string): Promise<EventHistoryResponse>;
  intervene(req: InterveneRequest): Promise<{ ok: true }>;
  dispatch(req: DispatchRequest): Promise<DispatchResponse>;
  vapidPub(): Promise<VapidPubResponse>;
  pushSubscribe(sub: PushSubscribeRequest): Promise<{ ok: true }>;
  openConvSocket(): WebSocket;
  openTaskSocket(taskId: string): WebSocket;
}

const jsonHeaders = { "content-type": "application/json" };

async function jsonFetch<T>(input: RequestInfo, init?: RequestInit): Promise<T> {
  const res = await fetch(input, {
    credentials: "include",
    ...init,
    headers: { ...jsonHeaders, ...(init?.headers ?? {}) },
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new ApiError(res.status, body || res.statusText);
  }
  return (await res.json()) as T;
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message);
    this.name = "ApiError";
  }
}

function wsUrl(path: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}${path}`;
}

export function createHttpClient(): ApiClient {
  return {
    fetchTasks: (rootOnly) =>
      jsonFetch<TaskListResponse>(`/api/tasks${rootOnly ? "?root=1" : ""}`),
    fetchTaskEvents: (taskId, from) => {
      const q = from ? `?from=${encodeURIComponent(from)}` : "";
      return jsonFetch<EventHistoryResponse>(`/api/tasks/${encodeURIComponent(taskId)}/events${q}`);
    },
    intervene: (req) =>
      jsonFetch<{ ok: true }>(`/api/intervene`, { method: "POST", body: JSON.stringify(req) }),
    dispatch: (req) =>
      jsonFetch<DispatchResponse>(`/api/dispatch`, { method: "POST", body: JSON.stringify(req) }),
    vapidPub: () => jsonFetch<VapidPubResponse>(`/api/push/vapid-pub`),
    pushSubscribe: (sub) =>
      jsonFetch<{ ok: true }>(`/api/push/subscribe`, {
        method: "POST",
        body: JSON.stringify(sub),
      }),
    openConvSocket: () => new WebSocket(wsUrl("/api/conv")),
    openTaskSocket: (taskId) => new WebSocket(wsUrl(`/api/tasks/${encodeURIComponent(taskId)}/stream`)),
  };
}
