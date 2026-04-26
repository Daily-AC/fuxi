// 内部消息模型 · v2 阶段 2 · 仅 user + xuannv 两类（门客 / task card / tool call 留阶段 3）。
//
// 跟 EventKind 不是 1:1 ——
// - user: 我们自己 optimistic 插入；server 也会回 user_message，但用同 stableId 去重（v1 简化：
//   服务端 user_message 我们当 echo 忽略，避免重复渲染）
// - xuannv: agent_text_delta 累积到 last 玄女 bubble；agent_text/agent_responded 整段替换；
//   result_success / agent_idle / EndOfTurn 概念上完结当前 bubble（streaming=false）
//
// reduce 函数纯函数易测；UI 层只读 store 不知 EventKind 长什么样。

import type { EventKind } from "~/types/events";

export interface UserMessage {
  kind: "user";
  id: string;
  text: string;
  /** 503 重试期间 / 重试失败时挂 inline 错误。*/
  error?: string | null;
  /** 仍在等服务端回执（虚态），UI 可压暗或不挂时间戳。*/
  pending?: boolean;
  ts: number;
}

export interface XuannvMessage {
  kind: "xuannv";
  id: string;
  agent: string;
  text: string;
  /** true 时 bubble 末尾挂 pulse dot。*/
  streaming: boolean;
  ts: number;
}

export type Message = UserMessage | XuannvMessage;

// 玄女发言判定：role / agent 名字 fallback 到"是否 xuannv"
function isXuannv(agent: string | undefined): boolean {
  if (!agent) return false;
  return agent === "xuannv" || agent === "玄女" || agent.startsWith("xuannv");
}

/** 从 EventKind 应用一条事件到 messages 列表，返回新列表。
 *  纯函数 —— 调用方负责赋回 store。*/
export function applyEvent(prev: Message[], ev: EventKind): Message[] {
  const kind = (ev as { type: string }).type;
  const ts = parseTs(ev.ts);

  // 玄女流式增量
  if (kind === "agent_text_delta") {
    const e = ev as { agent: string; delta: string; id?: string };
    if (!isXuannv(e.agent)) return prev;
    const last = prev[prev.length - 1];
    if (last && last.kind === "xuannv" && last.streaming && last.agent === e.agent) {
      const updated: XuannvMessage = { ...last, text: last.text + e.delta };
      return [...prev.slice(0, -1), updated];
    }
    // 没有进行中的 bubble：起新一个
    const next: XuannvMessage = {
      kind: "xuannv",
      id: e.id ?? `xn-${ts}-${prev.length}`,
      agent: e.agent,
      text: e.delta,
      streaming: true,
      ts,
    };
    return [...prev, next];
  }

  // 玄女整段（agent_text / agent_responded）—— 整段到达视为完结当前 bubble
  if (kind === "agent_text" || kind === "agent_responded") {
    const e = ev as { agent: string; text: string; id?: string };
    if (!isXuannv(e.agent)) return prev;
    const last = prev[prev.length - 1];
    if (last && last.kind === "xuannv" && last.streaming && last.agent === e.agent) {
      // 同一 turn 的 final text，覆盖累积值（整段权威）
      const updated: XuannvMessage = { ...last, text: e.text, streaming: false };
      return [...prev.slice(0, -1), updated];
    }
    const next: XuannvMessage = {
      kind: "xuannv",
      id: e.id ?? `xn-${ts}-${prev.length}`,
      agent: e.agent,
      text: e.text,
      streaming: false,
      ts,
    };
    return [...prev, next];
  }

  // 完结信号：标当前 streaming=false（不动 text）
  if (
    kind === "agent_idle" ||
    kind === "result_success" ||
    kind === "result_error" ||
    kind === "task_completed"
  ) {
    const last = prev[prev.length - 1];
    if (last && last.kind === "xuannv" && last.streaming) {
      const updated: XuannvMessage = { ...last, streaming: false };
      return [...prev.slice(0, -1), updated];
    }
    return prev;
  }

  // 服务端 echo 的 user_message：v1 简化 —— 忽略，optimistic 已经渲染
  if (kind === "user_message") return prev;

  // 阶段 3 才管 tool_call / task_created / 门客 message —— 阶段 2 全 noop
  return prev;
}

function parseTs(iso?: string): number {
  if (!iso) return Date.now();
  const t = new Date(iso).getTime();
  return Number.isNaN(t) ? Date.now() : t;
}

/** optimistic user message factory。*/
export function makeUserMessage(text: string): UserMessage {
  return {
    kind: "user",
    id: `u-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    text,
    pending: true,
    ts: Date.now(),
  };
}

/** 标记 user message 完成 / 失败。返回新数组。*/
export function markUserMessage(
  prev: Message[],
  id: string,
  patch: Partial<Pick<UserMessage, "pending" | "error">>,
): Message[] {
  return prev.map((m) => (m.kind === "user" && m.id === id ? { ...m, ...patch } : m));
}
