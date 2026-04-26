// 内部消息模型 · v2 阶段 2（wire fix） · 仅 user + xuannv 两类。
//
// **wire format 关键**：后端 γ 推的事件是 `{ meta, kind }` 嵌套：
//   { meta: { id, at, agent, task, ... }, kind: { type: "agent_responded", text: "..." } }
// agent uuid / task uuid / 时间戳都在 meta；事件 payload 在 kind（discriminated by type）。
// reducer 必须用 `ev.kind.type` / `ev.meta.agent`，不是 flat。
//
// 玄女判定：
//   meta.agent 是 uuid（不是 "xuannv" 字符串），无法靠名字判。v1 简化策略：
//   conv WS 推过来的事件**都视为玄女主线**（因为 conv 端点本就只过滤"跟玄女对话"的事件流），
//   将来 γ broaden 加门客后再扩 reducer。
//
// 后端实际事件：
//   - user_intervention_sent · 我们 optimistic 已渲染，忽略
//   - thinking_started · streaming 开始（玄女在想）
//   - agent_responded · 整段回复，完结当前 streaming bubble
//   - thinking_finished / agent_idle / result_success / task_completed · 完结
//   - agent_text_delta · 真发 delta 时累积（cc haiku 当前不发，留给将来 sonnet/opus）
//   - 其它（task_created / tool_call / custom / ...）阶段 2 noop，留阶段 3

import type { ServerEvent } from "~/types/events";

export interface UserMessage {
  kind: "user";
  id: string;
  text: string;
  /** 503 重试 / 重试失败时挂 inline 错误。*/
  error?: string | null;
  /** 仍在等服务端回执（虚态）。*/
  pending?: boolean;
  ts: number;
}

export interface XuannvMessage {
  kind: "xuannv";
  id: string;
  /** agent uuid（debug 用；UI 不暴露）。*/
  agent: string | null;
  text: string;
  /** true 时 bubble 末尾挂 pulse dot。*/
  streaming: boolean;
  ts: number;
}

export type Message = UserMessage | XuannvMessage;

function parseTs(iso?: string | null): number {
  if (!iso) return Date.now();
  const t = new Date(iso).getTime();
  return Number.isNaN(t) ? Date.now() : t;
}

function lastStreamingXuannv(prev: Message[]): XuannvMessage | null {
  const last = prev[prev.length - 1];
  return last && last.kind === "xuannv" && last.streaming ? last : null;
}

/** 起一个新的玄女 streaming bubble（thinking_started / 首段 delta 时用）。*/
function startBubble(prev: Message[], ev: ServerEvent, initialText: string): Message[] {
  const ts = parseTs(ev.meta.at);
  const next: XuannvMessage = {
    kind: "xuannv",
    id: ev.meta.id || `xn-${ts}-${prev.length}`,
    agent: ev.meta.agent ?? null,
    text: initialText,
    streaming: true,
    ts,
  };
  return [...prev, next];
}

/** 把 ServerEvent 应用到 messages 列表，返回新列表。纯函数。*/
export function applyEvent(prev: Message[], ev: ServerEvent): Message[] {
  // 防御：wire 偶发缺字段时不要崩
  if (!ev || !ev.kind || typeof ev.kind.type !== "string") return prev;
  const k = ev.kind;

  // —— 流式开始：thinking_started ——
  // 玄女开始"想"了，立即起一个空白 streaming bubble 让用户看到 pulse。
  if (k.type === "thinking_started") {
    // 已经有进行中的 bubble 就不重起
    if (lastStreamingXuannv(prev)) return prev;
    return startBubble(prev, ev, "");
  }

  // —— 流式增量：agent_text_delta ——
  if (k.type === "agent_text_delta") {
    const delta = (k as { delta?: string }).delta ?? "";
    const last = lastStreamingXuannv(prev);
    if (last) {
      const updated: XuannvMessage = { ...last, text: last.text + delta };
      return [...prev.slice(0, -1), updated];
    }
    return startBubble(prev, ev, delta);
  }

  // —— 整段：agent_text / agent_responded ——
  // 整段到达视为完结当前 bubble；如果没有进行中的，直接起一个非 streaming 的。
  if (k.type === "agent_text" || k.type === "agent_responded") {
    const text = (k as { text?: string }).text ?? "";
    const last = lastStreamingXuannv(prev);
    if (last) {
      const updated: XuannvMessage = { ...last, text, streaming: false };
      return [...prev.slice(0, -1), updated];
    }
    const ts = parseTs(ev.meta.at);
    const next: XuannvMessage = {
      kind: "xuannv",
      id: ev.meta.id || `xn-${ts}-${prev.length}`,
      agent: ev.meta.agent ?? null,
      text,
      streaming: false,
      ts,
    };
    return [...prev, next];
  }

  // —— 完结信号：thinking_finished / agent_idle / result_success / result_error / task_completed ——
  // 标当前 streaming=false（不动 text）。如果当前 bubble 还是空（仅 thinking_started 起来过
  // 但还没收到任何 delta / responded），把它丢掉避免空白 bubble 残留。
  if (
    k.type === "thinking_finished" ||
    k.type === "agent_idle" ||
    k.type === "result_success" ||
    k.type === "result_error" ||
    k.type === "task_completed"
  ) {
    const last = lastStreamingXuannv(prev);
    if (!last) return prev;
    if (last.text === "") {
      // 空 bubble · 丢弃
      return prev.slice(0, -1);
    }
    const updated: XuannvMessage = { ...last, streaming: false };
    return [...prev.slice(0, -1), updated];
  }

  // —— 服务端 echo 自己的 user_intervention_sent · optimistic 已渲染 ——
  if (k.type === "user_intervention_sent") return prev;

  // —— 阶段 3 才管 ——
  return prev;
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
