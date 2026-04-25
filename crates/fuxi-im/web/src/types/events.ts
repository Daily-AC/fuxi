// EventKind wire 格式 —— 跟 fuxi-core 的 #[serde(tag="type")] 联合一一对应。
// 决策 14 §C：TUI 和 PWA 共用同一份 EventKind，新增变体两边同步。
// 这里用宽松联合 + 兜底 Unknown，便于后端先行 / PWA 后行。

export type AgentId = string;
export type TaskId = string;
export type Iso = string;

export interface BaseMeta {
  id?: string;
  ts?: Iso;
  task_id?: TaskId | null;
  agent_id?: AgentId | null;
}

export type EventKind =
  | ({ type: "agent_spawning"; agent: AgentId; role?: string } & BaseMeta)
  | ({ type: "agent_ready"; agent: AgentId; role?: string } & BaseMeta)
  | ({ type: "agent_idle"; agent: AgentId } & BaseMeta)
  | ({ type: "agent_busy"; agent: AgentId } & BaseMeta)
  | ({ type: "agent_shutdown"; agent: AgentId; reason?: string } & BaseMeta)
  | ({ type: "task_created"; task: TaskId; title: string; parent?: TaskId | null } & BaseMeta)
  | ({ type: "task_dispatched"; task: TaskId; agent: AgentId } & BaseMeta)
  | ({ type: "task_completed"; task: TaskId; ok: boolean; summary?: string } & BaseMeta)
  | ({ type: "task_failed"; task: TaskId; error: string } & BaseMeta)
  | ({ type: "user_message"; text: string; to?: AgentId } & BaseMeta)
  | ({ type: "agent_text_delta"; agent: AgentId; delta: string } & BaseMeta)
  | ({ type: "agent_text"; agent: AgentId; text: string } & BaseMeta)
  | ({ type: "agent_responded"; agent: AgentId; text: string } & BaseMeta)
  | ({ type: "tool_call"; agent: AgentId; tool: string; input?: unknown } & BaseMeta)
  | ({ type: "tool_result"; agent: AgentId; tool: string; output?: unknown; ok: boolean } & BaseMeta)
  | ({ type: "skill_invoked"; agent: AgentId; skill: string } & BaseMeta)
  | ({ type: "diff"; path: string; before?: string; after?: string } & BaseMeta)
  | ({ type: "result_success"; agent: AgentId; summary?: string } & BaseMeta)
  | ({ type: "result_error"; agent: AgentId; error: string } & BaseMeta)
  | ({ type: string; [k: string]: unknown } & BaseMeta);

export interface TaskCard {
  id: TaskId;
  title: string;
  status: "pending" | "running" | "done" | "failed" | "blocked";
  created_at: Iso;
  updated_at: Iso;
  agent?: AgentId | null;
  parent?: TaskId | null;
  summary?: string | null;
  // ε 用：上次本地看到的最后一条事件 id，作为 cursor
  last_seen_event?: string | null;
}

export interface TaskListResponse {
  tasks: TaskCard[];
}

export interface EventHistoryResponse {
  events: EventKind[];
  next_cursor?: string | null;
}

export interface InterveneRequest {
  text: string;
  task_id?: TaskId | null;
}

export interface DispatchRequest {
  title: string;
  text: string;
  role?: string;
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
