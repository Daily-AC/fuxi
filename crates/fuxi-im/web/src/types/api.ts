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

/** intervene 阶段 3 扩 attachments 字段（β 配合）。*/
export interface InterveneRequestV2 {
  text: string;
  task_id?: TaskId | null;
  attachments?: string[];
}

// ---------- 阶段 4 · 任务 sheet 视图模型（β #21 契约目标） ----------

export type TaskMemberStatus = "busy" | "idle" | "thinking";
export type TaskGroupStatus = "running" | "completed" | "failed";

/** 任务成员 · 每行渲染 [dot · 角色名 · 当前 activity · tokens] */
export interface TaskMember {
  agent_id: string;
  role: string; // role key: "luban" / "pusong" / ...
  role_display: string; // "鲁班" / "蒲松" / "玄女"
  activity?: string | null; // 当前 tool call 短描述
  tokens?: number | null; // 累积 tokens
  status: TaskMemberStatus;
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
}

export interface TasksOverview {
  running: TaskGroupCard[];
  completed: TaskGroupCard[];
}
