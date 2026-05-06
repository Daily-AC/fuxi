// ServerEvent wire 格式 —— 跟后端 γ 的 fuxi_events::Event { meta, kind } 一一对应。
// 实测（2026-04-26）：γ 直接 serde_json::to_string(&event)，wire 是嵌套不是 flat：
// { "meta": { id, at, session, agent, task }, "kind": { "type": "...", ... } }
// flat 假设是 v1 fixture 的误导，已纠正。

export type AgentId = string;
export type TaskId = string;
export type Iso = string;

/** 事件元数据（顶层）。所有事件都带这一段。*/
export interface EventMeta {
  id: string;
  at: Iso;
  /** 会话 id（Decision 12 dist worker 引入；v1 单节点可为 null）。*/
  session?: string | null;
  /** 发出该事件的 agent uuid。"玄女" / "鲁班" 是 role；agent 字段是 uuid。*/
  agent?: AgentId | null;
  /** 关联的 task uuid（如有）。*/
  task?: TaskId | null;
}

/** 事件 payload union · #[serde(tag="type")]
 *  实测 γ 推过的：user_intervention_sent / task_created / task_dispatched /
 *  agent_ready / thinking_started / thinking_finished / agent_responded /
 *  custom / task_state_changed
 *  暂未见到 agent_text_delta（cc haiku 模型不发 delta，只发整段 agent_responded）。
 */
export type EventKind =
  | {
      type: "user_intervention_sent";
      target?: string;
      mode?: string;
      text: string;
      mentions?: string[];
      pinned_node?: string | null;
      /** 阶段 3 attachments 管道 · 后端 EventKind::UserInterventionSent.attachments
       *  序列化产出。每条是 uploads 表 id；前端拼直链 `/api/uploads/:id` 渲缩略。 */
      attachments?: string[];
    }
  | { type: "task_created"; title?: string; parent?: TaskId | null }
  | { type: "task_dispatched"; agent?: AgentId | null }
  | { type: "task_state_changed"; from?: string; to: string }
  | { type: "task_completed"; ok?: boolean; summary?: string }
  | { type: "task_failed"; error?: string }
  | { type: "agent_spawning"; role?: string }
  | { type: "agent_ready"; role?: string }
  | { type: "agent_idle" }
  | { type: "agent_busy" }
  | { type: "agent_shutdown"; reason?: string }
  | { type: "thinking_started" }
  | { type: "thinking_finished" }
  | { type: "agent_text_delta"; delta: string }
  | { type: "agent_text"; text: string }
  | { type: "agent_responded"; text: string }
  | { type: "tool_call"; tool: string; input?: unknown }
  | { type: "tool_result"; tool: string; output?: unknown; ok: boolean }
  | { type: "custom"; [k: string]: unknown }
  | { type: "result_success"; summary?: string }
  | { type: "result_error"; error: string }
  // 兜底：未知事件（forward-compat，让 reducer noop）
  | { type: string; [k: string]: unknown };

/** 完整事件 wire 形态。*/
export interface ServerEvent {
  meta: EventMeta;
  kind: EventKind;
}

// ---------- 任务列表 / 历史回放 / 鉴权 / push 等 REST 类型保持不变 ----------

export interface TaskCard {
  id: TaskId;
  title: string;
  status: "pending" | "running" | "done" | "failed" | "blocked";
  created_at: Iso;
  updated_at: Iso;
  agent?: AgentId | null;
  parent?: TaskId | null;
  summary?: string | null;
  last_seen_event?: string | null;
}

export interface TaskListResponse {
  tasks: TaskCard[];
}

export interface EventHistoryResponse {
  events: ServerEvent[];
  next_cursor?: string | null;
}

export interface InterveneRequest {
  text: string;
  task_id?: TaskId | null;
  /** 阶段 3：上传文件 id 列表（uploads 表 uuid）。后端拼到玄女文本里给她 Read。*/
  attachments?: string[];
}

/** POST /api/dispatch 请求体——后端 `handlers::dispatch::DispatchBody`。
 *
 *  v2 跨节点 sandbox：可选 `project` 把 task 关联到已注册项目；
 *  Fuxi::dispatch 会按 project.host_nodes auto-pin 到最闲节点 + worker 端
 *  resolve_project_sandbox_cwd 把 cc/codex spawn 进对应 sandbox。
 *
 *  PWA composer @<slug> 解析后走这条路（不走 intervene），避免玄女把整段
 *  prompt 又转一遍——dispatch 直接进玄女、auto-pin 路由 + dist enqueue。 */
export interface DispatchRequest {
  /** 短标题（PWA 任务卡显示）。 */
  title: string;
  /** 任务详情（玄女 / 门客读到的具体诉求）。 */
  description: string;
  /** v2 跨节点：已注册项目 slug。无值 → 不绑项目（v1 兼容）。 */
  project?: string;
}

export interface DispatchResponse {
  task_id: TaskId;
}

export interface PushSubscribeRequest {
  endpoint: string;
  keys: { p256dh: string; auth: string };
}

export interface VapidPubResponse {
  public_key: string;
}
