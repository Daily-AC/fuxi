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

/** intervene 阶段 3 扩 attachments 字段（β 配合）。
 *  阶段 #N3 扩 `target`：私聊页发给特定 worker 时带 worker agent uuid；
 *  缺省（玄女主对话）走老路径不带 target。β #N5 落后才真消费这个字段。*/
export interface InterveneRequestV2 {
  text: string;
  task_id?: TaskId | null;
  attachments?: string[];
  /** 私聊页 → worker agent uuid。玄女主对话留空。*/
  target?: string;
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
