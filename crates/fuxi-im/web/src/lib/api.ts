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
  AcceptDeliverableRequest,
  AddProjectRequest,
  AddProjectResponse,
  ConversationHistoryResponse,
  CreateTopicRequest,
  CronResponse,
  CurrentTopicResponse,
  DeliverablesResponse,
  EphemeralResponse,
  InterveneRequestV2,
  MemoryResponse,
  NodesResponse,
  NotificationsResponse,
  ProjectsResponse,
  RejectDeliverableRequest,
  RolesResponse,
  SandboxesResponse,
  TasksOverview,
  TopicView,
  TopicsResponse,
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

/** GET /api/voice/tokens——cookie 登录态换语音三件套 token。
 *  wake_token=null 表示 home 未部署唤醒服务（UI 隐藏语音模式开关，PTT 仍可用）。*/
export interface VoiceTokensResponse {
  im_token: string;
  wake_token: string | null;
}

export interface ApiClient {
  fetchTasks(rootOnly: boolean): Promise<TaskListResponse>;
  fetchTaskEvents(taskId: string, fromCursor?: string): Promise<EventHistoryResponse>;
  /** 阶段 3 升级：intervene 接受可选 attachments。后端 β #17 兼容旧无 attachments。*/
  intervene(req: InterveneRequest | InterveneRequestV2): Promise<{ ok: true }>;
  /** PWA 语音：登录态换 asr/tts/wake token（src/voice/ 模块直连各代理用）。*/
  voiceTokens(): Promise<VoiceTokensResponse>;
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
  /** Decision 21 phase 1 · POST /api/projects · 注册新项目。
   *  失败映射：409 conflict / 400 bad request（非 git repo / 非法 slug）。 */
  addProject(req: AddProjectRequest): Promise<AddProjectResponse>;
  /** Decision 21 phase 1 · DELETE /api/projects/:id · 删项目（连带 sandboxes 等）。
   *  失败映射：404 not found / 400 非法 id。 */
  removeProject(id: string): Promise<void>;
  /** Decision 21 phase 1 · GET /api/projects/{id}/sandboxes · 列项目的 L3 sandboxes。 */
  fetchSandboxes(projectId: string): Promise<SandboxesResponse>;
  /** Decision 21 phase 3 · GET /api/projects/{id}/ephemeral · L2 active + archived。 */
  fetchEphemeral(projectId: string): Promise<EphemeralResponse>;
  /** Decision 22 phase 1 · GET /api/deliverables · 跨项目交付收件箱（按 produced_at 倒序）。 */
  fetchDeliverables(): Promise<DeliverablesResponse>;
  /** Decision 22 phase 1 · 拼接交付文件直链 URL，给 <a href> / window.open 用。
   *  注意 task 在 backend wire 是裸 uuid，但路由用的是 `task-<uuid>`——前端拼时加前缀。 */
  deliverableFileUrl(project: string, task: string, name: string): string;
  /** Decision 22 phase 3 · inline preview URL（不设 Content-Disposition: attachment）。
   *  bug #76：用户期望卡片打开后内联预览（md 渲染、img 直显），下载按钮显式触发。 */
  deliverableFilePreviewUrl(project: string, task: string, name: string): string;
  /** Decision 22 phase 2 · 接收交付（POST → 后端 publish DeliverableAccepted 事件）。 */
  acceptDeliverable(
    project: string,
    task: string,
    req?: AcceptDeliverableRequest,
  ): Promise<void>;
  /** Decision 22 phase 2 · 拒绝交付。 */
  rejectDeliverable(
    project: string,
    task: string,
    req?: RejectDeliverableRequest,
  ): Promise<void>;
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
  /** dist topology 实时变更流——节点 tab 订阅，每条 push 触发 fetchNodes refetch。
   *  filter：WorkerRegistered / WorkerHeartbeatStateChanged / WorkerStaleSwept 三类。 */
  openNodesStreamSocket(): WebSocket;
  /** v1-session16 · 通知 tab · GET /api/notifications · bug + handoff offer + review。 */
  fetchNotifications(opts?: {
    kind?: string;
    /** v1-session19 · status 过滤（'open' / 'awaiting_test' / 'closed'）。 */
    status?: string;
    include_closed?: boolean;
    limit?: number;
  }): Promise<NotificationsResponse>;
  /** 标已读（仅清红点，仍在列表里）。 */
  markNotificationRead(id: string): Promise<{ ok: true }>;
  /** 关闭（默认列表隐藏）。actor 默认 "user"；body 可携 note。 */
  closeNotification(id: string, opts?: { actor?: string; note?: string }): Promise<{ ok: true }>;
  /** v1-session19 · 重开 issue。 */
  reopenNotification(id: string, opts?: { actor?: string; note?: string }): Promise<{ ok: true }>;
  /** 一键全部标已读——进入「通知」tab 时调，红点清零。 */
  readAllNotifications(): Promise<{ ok: true; updated: number }>;
  /** v1-session17 task #9 · 「更多 → 记忆」· GET /api/memory · 策府现行事实分组。 */
  fetchMemory(opts?: { limit?: number }): Promise<MemoryResponse>;
  /** v1-session17 task #9 · 「更多 → 角色」· GET /api/roles · roles 目录扫描结果。 */
  fetchRoles(): Promise<RolesResponse>;
  /** v1-session17 task #9 · 「更多 → 更漏」· GET /api/cron · scheduler trigger 列表。 */
  fetchCron(): Promise<CronResponse>;
  /** Phase 1 · 列 topic 加当前 topic_id（sidebar 数据源）。 */
  fetchTopics(includeArchived?: boolean): Promise<TopicsResponse>;
  /** Phase 1 · 单独读当前 topic_id（PWA 对账兜底）。 */
  fetchCurrentTopic(): Promise<CurrentTopicResponse>;
  /** Phase 1 · 建 topic（title trim 后非空、≤80 字；后端 400 拒）。 */
  createTopic(req: CreateTopicRequest): Promise<TopicView>;
  /** Phase 1 · 切玄女当前 topic。后端走 shutdown_xuannv_for_handoff + spawn 新 cc，
   *  实际可能 5-15s——调用方必管 loading 态。返回切完后的 current_topic_id。 */
  switchTopic(id: string): Promise<CurrentTopicResponse>;
  /** Phase 1 · 归档 topic（不删，include_archived=1 才能再列出）。 */
  archiveTopic(id: string): Promise<void>;
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
    voiceTokens: () => jsonFetch<VoiceTokensResponse>(`/api/voice/tokens`),
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
    addProject: (req) =>
      jsonFetch<AddProjectResponse>(`/api/projects`, {
        method: "POST",
        body: JSON.stringify(req),
      }),
    removeProject: async (id) => {
      const res = await fetch(`/api/projects/${encodeURIComponent(id)}`, {
        method: "DELETE",
        credentials: "include",
      });
      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new ApiError(res.status, body || res.statusText);
      }
    },
    fetchSandboxes: (projectId) =>
      jsonFetch<SandboxesResponse>(
        `/api/projects/${encodeURIComponent(projectId)}/sandboxes`,
      ),
    fetchEphemeral: (projectId) =>
      jsonFetch<EphemeralResponse>(
        `/api/projects/${encodeURIComponent(projectId)}/ephemeral`,
      ),
    fetchDeliverables: () => jsonFetch<DeliverablesResponse>(`/api/deliverables`),
    deliverableFileUrl: (project, task, name) => {
      // backend wire 用裸 uuid，路由要 `task-<uuid>` 前缀；统一在前端拼合让调用方
      // 直接传 task uuid 拼下载 URL 不易出错。
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      return `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/files/${encodeURIComponent(name)}`;
    },
    deliverableFilePreviewUrl: (project, task, name) => {
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      return `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/preview/${encodeURIComponent(name)}`;
    },
    acceptDeliverable: async (project, task, req) => {
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      const url = `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/accept`;
      const res = await fetch(url, {
        method: "POST",
        credentials: "include",
        headers: jsonHeaders,
        body: JSON.stringify(req ?? {}),
      });
      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new ApiError(res.status, body || res.statusText);
      }
    },
    rejectDeliverable: async (project, task, req) => {
      const taskWithPrefix = task.startsWith("task-") ? task : `task-${task}`;
      const url = `/api/deliverables/${encodeURIComponent(project)}/${encodeURIComponent(taskWithPrefix)}/reject`;
      const res = await fetch(url, {
        method: "POST",
        credentials: "include",
        headers: jsonHeaders,
        body: JSON.stringify(req ?? {}),
      });
      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new ApiError(res.status, body || res.statusText);
      }
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
    openNodesStreamSocket: () => new WebSocket(wsUrl("/api/nodes/stream")),
    fetchNotifications: (opts) => {
      const q: string[] = [];
      if (opts?.kind) q.push(`kind=${encodeURIComponent(opts.kind)}`);
      if (opts?.status) q.push(`status=${encodeURIComponent(opts.status)}`);
      if (opts?.include_closed) q.push("include_closed=true");
      if (opts?.limit) q.push(`limit=${opts.limit}`);
      const qs = q.length > 0 ? `?${q.join("&")}` : "";
      return jsonFetch<NotificationsResponse>(`/api/notifications${qs}`);
    },
    markNotificationRead: (id) =>
      jsonFetch<{ ok: true }>(`/api/notifications/${encodeURIComponent(id)}/read`, {
        method: "POST",
      }),
    closeNotification: (id, opts) =>
      jsonFetch<{ ok: true }>(`/api/notifications/${encodeURIComponent(id)}/close`, {
        method: "POST",
        body: JSON.stringify(opts ?? {}),
      }),
    reopenNotification: (id, opts) =>
      jsonFetch<{ ok: true }>(`/api/notifications/${encodeURIComponent(id)}/reopen`, {
        method: "POST",
        body: JSON.stringify(opts ?? {}),
      }),
    readAllNotifications: () =>
      jsonFetch<{ ok: true; updated: number }>(`/api/notifications/read-all`, {
        method: "POST",
      }),
    fetchMemory: (opts) => {
      const qs = opts?.limit ? `?limit=${opts.limit}` : "";
      return jsonFetch<MemoryResponse>(`/api/memory${qs}`);
    },
    fetchRoles: () => jsonFetch<RolesResponse>(`/api/roles`),
    fetchCron: () => jsonFetch<CronResponse>(`/api/cron`),
    fetchTopics: (includeArchived) => {
      const qs = includeArchived ? "?include_archived=1" : "";
      return jsonFetch<TopicsResponse>(`/api/topics${qs}`);
    },
    fetchCurrentTopic: () => jsonFetch<CurrentTopicResponse>(`/api/topics/current`),
    createTopic: (req) =>
      jsonFetch<TopicView>(`/api/topics`, {
        method: "POST",
        body: JSON.stringify(req),
      }),
    switchTopic: (id) =>
      jsonFetch<CurrentTopicResponse>(`/api/topics/${encodeURIComponent(id)}/switch`, {
        method: "POST",
      }),
    archiveTopic: async (id) => {
      // 204 No Content——jsonFetch 会 await res.json() 撞空 body，单独 fetch。
      const res = await fetch(`/api/topics/${encodeURIComponent(id)}/archive`, {
        method: "POST",
        credentials: "include",
      });
      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new ApiError(res.status, body || res.statusText);
      }
    },
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
