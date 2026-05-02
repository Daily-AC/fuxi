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
import type {
  ConversationHistoryResponse,
  DeliverablesResponse,
  InterveneRequestV2,
  NodesResponse,
  ProjectsResponse,
  TasksOverview,
  Upload,
} from "~/types/api";

export interface LoginRequest {
  password: string;
  device_name: string;
}

export interface PairRequest {
  pin: string;
  device_name: string;
}

/** 鉴权结果——成功后 cookie 由后端 Set-Cookie 设，body 只回 device_id（debug + 吊销）。*/
export interface AuthResponse {
  device_id: string;
}

export interface ApiClient {
  fetchTasks(rootOnly: boolean): Promise<TaskListResponse>;
  fetchTaskEvents(taskId: string, fromCursor?: string): Promise<EventHistoryResponse>;
  /** 阶段 3 升级：intervene 接受可选 attachments。后端 β #17 兼容旧无 attachments。*/
  intervene(req: InterveneRequest | InterveneRequestV2): Promise<{ ok: true }>;
  dispatch(req: DispatchRequest): Promise<DispatchResponse>;
  vapidPub(): Promise<VapidPubResponse>;
  pushSubscribe(sub: PushSubscribeRequest): Promise<{ ok: true }>;
  /** 主路：主密码登入。401 密码错；503 服务端没设密码；429 过多尝试；200 成功。*/
  login(req: LoginRequest): Promise<AuthResponse>;
  /** 副路：PIN 配对（"忘密码 / 没设过"降级路径）。*/
  pair(req: PairRequest): Promise<AuthResponse>;
  /** 拉持久化对话历史（从 im.db）。*/
  fetchHistory(convId: string, limit: number, before?: string): Promise<ConversationHistoryResponse>;
  /** 阶段 4 任务 sheet · 拉 running + completed 视图模型（β #21 目标契约）。*/
  fetchTasksOverview(): Promise<TasksOverview>;
  /** v3 #58 dist topology · GET /api/nodes（β #55 实装中，本接口契约由 spec 钉）。 */
  fetchNodes(): Promise<NodesResponse>;
  /** Decision 21 phase 1 · GET /api/projects · 注册过的项目列表。 */
  fetchProjects(): Promise<ProjectsResponse>;
  /** Decision 22 phase 1 · GET /api/deliverables · 跨项目交付收件箱（按 produced_at 倒序）。 */
  fetchDeliverables(): Promise<DeliverablesResponse>;
  /** Decision 22 phase 1 · 拼接交付文件直链 URL，给 <a href> / window.open 用。
   *  注意 task 在 backend wire 是裸 uuid，但路由用的是 `task-<uuid>`——前端拼时加前缀。 */
  deliverableFileUrl(project: string, task: string, name: string): string;
  /** #N3 私聊页 · 拉门客历史（β #N5 / #27 目标契约）。
   *  filter by meta.agent==agent_id；事件 kind 白名单见 spec §私聊页"事件 filter"。*/
  fetchWorkerEvents(agentId: string, fromCursor?: string): Promise<EventHistoryResponse>;
  /** 上传单文件，可选进度回调（0..1）。*/
  /** v3 #50: 支持 AbortSignal 取消上传中的 chip · removeAttach 时调 controller.abort() */
  uploadFile(
    file: File,
    onProgress?: (ratio: number) => void,
    signal?: AbortSignal,
  ): Promise<Upload>;
  openConvSocket(): WebSocket;
  openTaskSocket(taskId: string): WebSocket;
  /** #N3 私聊页 · 接门客流式事件（β #N5 / #27 目标契约）。*/
  openWorkerSocket(agentId: string): WebSocket;
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
    login: (req) =>
      jsonFetch<AuthResponse>(`/api/auth/login`, { method: "POST", body: JSON.stringify(req) }),
    pair: (req) =>
      jsonFetch<AuthResponse>(`/api/auth/pair`, { method: "POST", body: JSON.stringify(req) }),
    fetchHistory: (convId, limit, before) => {
      const params = new URLSearchParams({ conv: convId, limit: String(limit) });
      if (before) params.set("before", before);
      return jsonFetch<ConversationHistoryResponse>(`/api/conv/messages?${params.toString()}`);
    },
    fetchTasksOverview: () => jsonFetch<TasksOverview>(`/api/tasks`),
    fetchNodes: () => jsonFetch<NodesResponse>(`/api/nodes`),
    fetchProjects: () => jsonFetch<ProjectsResponse>(`/api/projects`),
    fetchDeliverables: () => jsonFetch<DeliverablesResponse>(`/api/deliverables`),
    deliverableFileUrl: (project, task, name) => {
      // backend wire 用裸 uuid，路由要 `task-<uuid>` 前缀；统一在前端拼合让调用方
      // 直接传 task uuid 拼下载 URL 不易出错。
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      return `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/files/${encodeURIComponent(name)}`;
    },
    fetchWorkerEvents: (agentId, from) => {
      const q = from ? `?from=${encodeURIComponent(from)}` : "";
      return jsonFetch<EventHistoryResponse>(
        `/api/workers/${encodeURIComponent(agentId)}/events${q}`,
      );
    },
    uploadFile: (file, onProgress, signal) => uploadViaXhr(file, onProgress, signal),
    openConvSocket: () => new WebSocket(wsUrl("/api/conv")),
    // v3 #N4'：任务 thread 走 /conv（白名单 filter）；老 /stream 留给 firehose 观察器
    openTaskSocket: (taskId) =>
      new WebSocket(wsUrl(`/api/tasks/${encodeURIComponent(taskId)}/conv`)),
    openWorkerSocket: (agentId) =>
      new WebSocket(wsUrl(`/api/workers/${encodeURIComponent(agentId)}/conv`)),
  };
}

/** XHR upload —— 用 XHR 而非 fetch 是为了 progress 事件（fetch 不支持上传进度）。
 *  v3 #50: signal 触发 abort → xhr.abort() → reject ApiError(0, "aborted")。*/
function uploadViaXhr(
  file: File,
  onProgress?: (ratio: number) => void,
  signal?: AbortSignal,
): Promise<Upload> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    const fd = new FormData();
    fd.append("file", file, file.name);
    xhr.open("POST", "/api/upload");
    xhr.withCredentials = true;
    xhr.responseType = "json";
    if (onProgress) {
      xhr.upload.addEventListener("progress", (e) => {
        if (e.lengthComputable && e.total > 0) onProgress(e.loaded / e.total);
      });
    }
    if (signal) {
      if (signal.aborted) {
        reject(new ApiError(0, "aborted"));
        return;
      }
      signal.addEventListener("abort", () => xhr.abort(), { once: true });
    }
    xhr.addEventListener("load", () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        resolve(xhr.response as Upload);
      } else {
        reject(new ApiError(xhr.status, xhr.statusText || "upload failed"));
      }
    });
    xhr.addEventListener("error", () => reject(new ApiError(0, "network error")));
    xhr.addEventListener("abort", () => reject(new ApiError(0, "aborted")));
    xhr.send(fd);
  });
}
