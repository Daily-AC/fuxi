// 历史 / 上传 / 附件 类型 · v2 阶段 3 · 跟 β #17 后端契约对齐
// im.db 持久化的对话消息 · 跟 EventBus 解耦（后者是事件流，im.db 是 chat 视图源）

import type { TaskId } from "./events";

export type ConversationRole = "user" | "xuannv" | "luban" | "pusong" | "system";
export type StoredMessageKind = "text" | "task_card" | "tool_call" | "file" | "error";

export interface StoredMessage {
  id: string;
  conv_id: string;
  role: ConversationRole;
  agent_id?: string | null;
  kind: StoredMessageKind;
  /** 按 kind 分支：text → string；file → { caption?: string }；其余阶段 4 再扩。*/
  content: unknown;
  attachments?: string[];
  source_event_id?: string | null;
  ts: string;
  task?: TaskId | null;
}

export interface ConversationHistoryResponse {
  messages: StoredMessage[];
  /** 用作下一页 fetch ?before= 的游标；null 表示已到顶。*/
  next_before?: string | null;
}

/** 上传成功后后端返回的文件元信息。直链是 GET /api/uploads/:id。*/
export interface Upload {
  id: string;
  name: string;
  mime: string;
  bytes: number;
  sha256: string;
}

/** intervene v3 spec 全字段（β #42/#N7' 已落契约）。
 *  - target：路由目标 agent_id；省 = 玄女默认；带 = 直走该 agent
 *  - mentions：所有 @ 的 agent_id（v3 chip composer 序列化），写入事件供历史还原 chip
 *  - attachments：阶段 3 文件附件
 *  历史 v2 的 InterveneRequest 不带 target 即等价 target 缺省。 */
export interface InterveneRequestV2 {
  text: string;
  task_id?: TaskId | null;
  attachments?: string[];
  /** 路由 target agent uuid。玄女主对话不带 = 默认走玄女。*/
  target?: string;
  /** v3 #N7' 加 · 所有 @ 的 agent_ids（含 target）。仅用于历史 chip 还原，不影响路由。*/
  mentions?: string[];
}

// ---------- 阶段 4 · 任务 sheet 视图模型（β #21 契约目标） ----------

export type TaskMemberStatus = "busy" | "idle" | "thinking";
export type TaskGroupStatus = "running" | "completed" | "failed";

/** 工具调用摘要 · 用于 task sheet 显 member 当前/最近一次工具（#26）。*/
export interface ToolCallSummary {
  tool: string; // "cargo test --lib" / "Read" / ...
  args_summary?: string | null; // 短描述（路径 / 参数）
  exit?: number | null; // exit code，null 表示仍在跑
  finished_at?: string | null;
  duration_ms?: number | null;
}

/** 任务成员 · 每行渲染 [dot · 角色名 · 当前 activity · tokens] */
export interface TaskMember {
  agent_id: string;
  role: string; // role key: "luban" / "pusong" / ...
  role_display: string; // "鲁班" / "蒲松" / "玄女"
  activity?: string | null; // 当前 tool call 短描述（旧字段，跟 last_tool_call.tool 同源）
  tokens?: number | null; // 累积 tokens
  status: TaskMemberStatus;
  /** #26 加 · 当前 / 最近一次工具调用详情。*/
  last_tool_call?: ToolCallSummary | null;
  /** 可选 · 最近 N 条工具调用（β 暂不返）。*/
  recent_tool_calls?: ToolCallSummary[];
}

/** 任务分组卡片 · 视图模型（不要跟 types/events.ts 的旧 TaskCard 混淆，那是 v1 主屏列表用的）。*/
export interface TaskGroupCard {
  id: string; // task uuid
  title: string;
  status: TaskGroupStatus;
  created_at: string; // ISO
  last_active_at: string;
  duration_ms: number;
  members: TaskMember[];
  /** #26 加 · 整 task 最近一条事件摘要（"鲁班 · cargo test --lib · exit 0"）。*/
  last_event_summary?: string | null;
}

export interface TasksOverview {
  running: TaskGroupCard[];
  completed: TaskGroupCard[];
}

// ---------- v3 #58 dist topology · GET /api/nodes 契约（β #55 实装中） ----------

export type NodeWorkerStatus = "busy" | "idle" | "thinking";

/** dist controller 维护的 node 上 worker 实例（dispatch 时记下，complete 时清）。*/
export interface NodeWorker {
  agent_id: string;
  role: string;
  role_display: string;
  status: NodeWorkerStatus;
  /** 当前在跑的 task uuid（idle 时 null）。*/
  current_task_id?: string | null;
  /** 当前在跑的 task title（idle 时 null）。*/
  current_task_title?: string | null;
}

/** dist topology 单节点视图。home 也走 dist register（特殊 node_id="home"）。*/
export interface NodeView {
  node_id: string;
  /** dist tags 数组（home 节点典型 ["home","linux"]，本地 ["local","mac",...]）。*/
  tags: string[];
  max_concurrency: number;
  inflight_jobs: number;
  /** dist heartbeat 距今 ms；> 30000 视为离线（backend 已计算 online）。*/
  heartbeat_lag_ms: number;
  online: boolean;
  registered_at?: string | null;
  workers: NodeWorker[];
}

export interface NodesResponse {
  nodes: NodeView[];
}
