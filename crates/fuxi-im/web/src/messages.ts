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
import type { ConversationRole, StoredMessage, Upload } from "~/types/api";

export interface UserMessage {
  kind: "user";
  id: string;
  text: string;
  /** 此条 user message 携带的附件（已上传完成）。*/
  attachments?: Upload[];
  /** v3 任务 thread · 该 turn @ 提及的所有 agent_ids（含 target 自身）。
   *  历史回放时用于还原 chip 视觉。本地 optimistic 也带（display 一致）。*/
  mentions?: string[];
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
  /** role 用于将来门客分支；阶段 3 仍主要走 xuannv。*/
  role?: ConversationRole;
  text: string;
  /** true 时 bubble 末尾挂 pulse dot。*/
  streaming: boolean;
  ts: number;
}

/** 文件消息 · 一条 message 包 N 个附件，可有/可无 caption 文字。*/
export interface FileMessage {
  kind: "file";
  id: string;
  role: ConversationRole;
  caption?: string;
  attachments: Upload[];
  ts: number;
}

/** 门客（worker）气泡 · v3 任务 thread #39 + v2 私聊页 #N3 共用。
 *  视觉：左对齐 + who 标签 + bubble，who 走 role 颜色 + role_display 渲染。 */
export interface WorkerMessage {
  kind: "worker";
  id: string;
  agent: string;
  /** role key（"luban" / "pusong" / ...）—— 用于 colorForRole 着色 who 标签。*/
  role?: string;
  /** UI 显示用的角色名（"鲁班" / "蒲松" / "墨子" ...）。*/
  role_display?: string;
  text: string;
  streaming: boolean;
  ts: number;
}

/** 工具调用卡 · ToolStarted/ToolFinished 配对折叠成单条 ToolCallMessage。
 *  tap 展开看 stdout（前 20 行截断）。
 *  v1 实装只显折叠条 + 状态点；展开 stdout 留交互 stub（只读 output 字段）。*/
export interface ToolCallMessage {
  kind: "tool_call";
  id: string;
  agent: string;
  tool: string;
  args_summary?: string | null;
  status: "running" | "ok" | "err";
  duration_ms?: number | null;
  /** 简化：只存最后一次 ToolFinished 的 output。*/
  output?: string | null;
  ts: number;
}

/** Thinking 折叠条 · 灰色斜体 `▸ 思考 Ns`，tap 展开。
 *  ThinkingStarted 起一条 streaming，ThinkingDone 标完结 + duration。*/
export interface ThinkingMessage {
  kind: "thinking";
  id: string;
  agent: string;
  duration_ms?: number | null;
  streaming: boolean;
  ts: number;
}

/** 状态变更 marker · 居中 muted 行 `─ 鲁班 idle ─`。
 *  来源：task_completed / agent_idle（filter 内白名单）。*/
export interface StatusMarker {
  kind: "marker";
  id: string;
  text: string;
  ts: number;
}

export type Message =
  | UserMessage
  | XuannvMessage
  | FileMessage
  | WorkerMessage
  | ToolCallMessage
  | ThinkingMessage
  | StatusMarker;

/** StoredMessage.content 的 text 抽取 · 兼容两路 wire：
 *  - backend serde_json::json!({"text":"..."}) 主路（β #17 实装至今）→ 取 .text
 *  - 裸字符串（早期或 mock）→ 直接返
 *  其它情况返空串（外层 .trim() === "" 会丢掉空 bubble）。 */
function textFromContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (content && typeof content === "object") {
    const t = (content as { text?: unknown }).text;
    if (typeof t === "string") return t;
  }
  return "";
}

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
  // Bug #24：text 为空（玄女这轮没产文字回复）→ 丢空 bubble，避免"玄女 16:55"灰圆残留。
  if (k.type === "agent_text" || k.type === "agent_responded") {
    const text = (k as { text?: string }).text ?? "";
    const isEmpty = text.trim() === "";
    const last = lastStreamingXuannv(prev);
    if (last) {
      if (isEmpty) {
        // streaming bubble 结束但没文字 → 丢
        return prev.slice(0, -1);
      }
      const updated: XuannvMessage = { ...last, text, streaming: false };
      return [...prev.slice(0, -1), updated];
    }
    if (isEmpty) return prev; // 没起 bubble + 空 text · noop
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
export function makeUserMessage(text: string, attachments?: Upload[]): UserMessage {
  return {
    kind: "user",
    id: `u-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    text,
    attachments,
    pending: true,
    ts: Date.now(),
  };
}

/** 把 im.db 历史的 StoredMessage 翻译成内部 Message。
 *  v1 简化：只识别 text / file；其它 kind（task_card / tool_call / error）阶段 4 处理。
 *
 *  Bug #24 防御：text 为空（包括只有空白字符）的玄女条目过滤掉，避免在 chat 里堆
 *  "玄女 16:55" 空灰圆 bubble。β #17 旧版 conv_store 把没回复的 thinking turn 也
 *  sync 进了 im.db；β #25 在做服务端侧过滤，前端这里是双保险。
 *  user 空 text 同样不应该出现，兜底丢。
 *
 *  Bug #45 修：backend conv_store::handle_event 写库时把 text kind 的 content 包成
 *  `{"text": "..."}` JSON 对象（serde_json::json! 宏），而不是裸字符串。
 *  这里 v3 之前一直用 `typeof s.content === "string"` 取值 → 永远拿到空串 → 历史消息
 *  全被丢掉，用户感知"刷新历史就没了"。修：兼容两种 wire（裸 string 兜底 + JSON 对象主路）。
 */
export function fromStoredMessage(s: StoredMessage): Message | null {
  const ts = parseTs(s.ts);
  if (s.kind === "text") {
    const text = textFromContent(s.content);
    if (text.trim() === "") return null;
    if (s.role === "user") {
      return {
        kind: "user",
        id: s.id,
        text,
        pending: false,
        ts,
      };
    }
    return {
      kind: "xuannv",
      id: s.id,
      agent: s.agent_id ?? null,
      role: s.role,
      text,
      streaming: false,
      ts,
    };
  }
  if (s.kind === "file") {
    const caption =
      s.content && typeof s.content === "object" && "caption" in s.content
        ? String((s.content as { caption?: string }).caption ?? "")
        : "";
    // attachments 在 history 里要么是 file_id refs 要么是完整 Upload[]；
    // β 给的是 file_id refs；ε 不知 mime/bytes 时显占位（阶段 4 完善）。
    const ups: Upload[] = Array.isArray(s.attachments)
      ? s.attachments.map((id) => ({ id, name: id, mime: "application/octet-stream", bytes: 0, sha256: "" }))
      : [];
    return {
      kind: "file",
      id: s.id,
      role: s.role,
      caption: caption || undefined,
      attachments: ups,
      ts,
    };
  }
  return null;
}

/** 历史预加载 + WS 流入去重：相同 id 的 message 不重复加。
 *  返回新数组，按 ts 排序（升序，老的在前）。*/
export function mergeMessages(prev: Message[], incoming: Message[]): Message[] {
  if (incoming.length === 0) return prev;
  const seen = new Set(prev.map((m) => m.id));
  const merged = [...prev];
  for (const m of incoming) {
    if (!seen.has(m.id)) {
      merged.push(m);
      seen.add(m.id);
    }
  }
  merged.sort((a, b) => a.ts - b.ts);
  return merged;
}

/** 标记 user message 完成 / 失败。返回新数组。*/
export function markUserMessage(
  prev: Message[],
  id: string,
  patch: Partial<Pick<UserMessage, "pending" | "error">>,
): Message[] {
  return prev.map((m) => (m.kind === "user" && m.id === id ? { ...m, ...patch } : m));
}

// ============================================================================
// 私聊页（#N3）专用 reducer · `applyWorkerEvent`
// ----------------------------------------------------------------------------
// 跟玄女 reducer 的差异：
//   - 单 agent 上下文：所有 bubble 都属于 props.agent；非该 agent 的事件直接丢
//     （后端 #N5 已 server-side filter，前端再防一次）
//   - AssistantText/agent_responded → WorkerMessage（不是 XuannvMessage）
//   - ToolStarted + ToolFinished 配对成 ToolCallMessage（折叠卡）
//   - ThinkingStarted/Done → ThinkingMessage 折叠条
//   - task_completed / agent_idle → StatusMarker 居中
//   - UserInterventionSent { target == agent } → 右侧 user bubble（用户自己发给该 worker）
// 渲染规则源：spec §"私聊页·渲染规则"
// ============================================================================

export interface WorkerReducerCtx {
  /** 该私聊页绑定的 worker agent uuid。 */
  agent: string;
  /** UI 显示名（"鲁班" 等），用于 WorkerMessage.role_display。可选。 */
  role_display?: string;
}

function lastStreamingWorker(prev: Message[]): WorkerMessage | null {
  const last = prev[prev.length - 1];
  return last && last.kind === "worker" && last.streaming ? last : null;
}

function lastStreamingThinking(prev: Message[]): ThinkingMessage | null {
  const last = prev[prev.length - 1];
  return last && last.kind === "thinking" && last.streaming ? last : null;
}

function startWorkerBubble(
  prev: Message[],
  ev: ServerEvent,
  ctx: WorkerReducerCtx,
  initialText: string,
): Message[] {
  const ts = parseTs(ev.meta.at);
  const next: WorkerMessage = {
    kind: "worker",
    id: ev.meta.id || `w-${ts}-${prev.length}`,
    agent: ctx.agent,
    role_display: ctx.role_display,
    text: initialText,
    streaming: true,
    ts,
  };
  return [...prev, next];
}

/** 找到 prev 里最近一条匹配 (agent, tool) 的 running ToolCallMessage 用于 ToolFinished 配对。
 *  没找到返 -1（finished 对不上 started 时降级直接插一条 ok 卡）。*/
function findRunningToolIdx(prev: Message[], agent: string, tool: string): number {
  for (let i = prev.length - 1; i >= 0; i -= 1) {
    const m = prev[i];
    if (
      m &&
      m.kind === "tool_call" &&
      m.agent === agent &&
      m.tool === tool &&
      m.status === "running"
    ) {
      return i;
    }
  }
  return -1;
}

export function applyWorkerEvent(
  prev: Message[],
  ev: ServerEvent,
  ctx: WorkerReducerCtx,
): Message[] {
  if (!ev || !ev.kind || typeof ev.kind.type !== "string") return prev;
  const k = ev.kind;
  const ts = parseTs(ev.meta.at);
  const evAgent = ev.meta.agent ?? null;

  // —— UserInterventionSent · 看 target 不看 meta.agent（spec §渲染规则） ——
  // 私聊页只接 target == ctx.agent 的；server-side 应已 filter，这里再防一次。
  if (k.type === "user_intervention_sent") {
    const target = (k as { target?: string }).target;
    if (target !== ctx.agent) return prev;
    const text = (k as { text?: string }).text ?? "";
    if (text.trim() === "") return prev;
    // optimistic 已渲染过的不重复（id 重复时 mergeMessages 会去重；这里直接 push 让 dedupe 处理）
    const id = ev.meta.id || `u-${ts}-${prev.length}`;
    const already = prev.some((m) => m.id === id);
    if (already) return prev;
    const next: UserMessage = {
      kind: "user",
      id,
      text,
      pending: false,
      ts,
    };
    return [...prev, next];
  }

  // 下面所有事件都要求 meta.agent == ctx.agent；不匹配丢
  if (evAgent !== ctx.agent) return prev;

  // —— 流式 thinking（折叠条 + 完结时填 duration） ——
  if (k.type === "thinking_started") {
    if (lastStreamingThinking(prev)) return prev;
    const id = ev.meta.id || `th-${ts}-${prev.length}`;
    const next: ThinkingMessage = {
      kind: "thinking",
      id,
      agent: ctx.agent,
      streaming: true,
      ts,
    };
    return [...prev, next];
  }
  if (k.type === "thinking_finished") {
    const last = lastStreamingThinking(prev);
    if (!last) return prev;
    const dur = ts - last.ts;
    const updated: ThinkingMessage = {
      ...last,
      streaming: false,
      duration_ms: dur >= 0 ? dur : null,
    };
    return [...prev.slice(0, -1), updated];
  }

  // —— 文本 delta / 整段 ——
  if (k.type === "agent_text_delta") {
    const delta = (k as { delta?: string }).delta ?? "";
    const last = lastStreamingWorker(prev);
    if (last) {
      const updated: WorkerMessage = { ...last, text: last.text + delta };
      return [...prev.slice(0, -1), updated];
    }
    return startWorkerBubble(prev, ev, ctx, delta);
  }
  if (k.type === "agent_text" || k.type === "agent_responded") {
    const text = (k as { text?: string }).text ?? "";
    const isEmpty = text.trim() === "";
    const last = lastStreamingWorker(prev);
    if (last) {
      if (isEmpty) return prev.slice(0, -1);
      const updated: WorkerMessage = { ...last, text, streaming: false };
      return [...prev.slice(0, -1), updated];
    }
    if (isEmpty) return prev;
    const next: WorkerMessage = {
      kind: "worker",
      id: ev.meta.id || `w-${ts}-${prev.length}`,
      agent: ctx.agent,
      role_display: ctx.role_display,
      text,
      streaming: false,
      ts,
    };
    return [...prev, next];
  }

  // —— 工具调用配对：tool_call + tool_result 折叠成单卡 ——
  // 注意：spec 用 ToolStarted/ToolFinished，wire 实际 EventKind 是 tool_call/tool_result
  // （types/events.ts §28），保持兼容。
  if (k.type === "tool_call") {
    const tool = (k as { tool?: string }).tool ?? "tool";
    const argsRaw = (k as { input?: unknown }).input;
    const args = stringifyArgs(argsRaw);
    const next: ToolCallMessage = {
      kind: "tool_call",
      id: ev.meta.id || `tc-${ts}-${prev.length}`,
      agent: ctx.agent,
      tool,
      args_summary: args,
      status: "running",
      ts,
    };
    return [...prev, next];
  }
  if (k.type === "tool_result") {
    const tool = (k as { tool?: string }).tool ?? "tool";
    const ok = Boolean((k as { ok?: boolean }).ok);
    const output = stringifyOutput((k as { output?: unknown }).output);
    const idx = findRunningToolIdx(prev, ctx.agent, tool);
    if (idx >= 0) {
      const running = prev[idx] as ToolCallMessage;
      const updated: ToolCallMessage = {
        ...running,
        status: ok ? "ok" : "err",
        duration_ms: ts - running.ts,
        output,
      };
      const next = prev.slice();
      next[idx] = updated;
      return next;
    }
    // 未配上 started · 直接插一条完结态卡
    const next: ToolCallMessage = {
      kind: "tool_call",
      id: ev.meta.id || `tc-${ts}-${prev.length}`,
      agent: ctx.agent,
      tool,
      status: ok ? "ok" : "err",
      duration_ms: 0,
      output,
      ts,
    };
    return [...prev, next];
  }

  // —— 状态变更 marker ——
  if (k.type === "agent_idle") {
    const text = `${ctx.role_display ?? "门客"} idle`;
    return appendMarker(prev, ev, text, ts);
  }
  if (k.type === "task_completed") {
    const summary = (k as { summary?: string }).summary;
    const text = summary ? `任务完成 · ${summary}` : "任务完成";
    return appendMarker(prev, ev, text, ts);
  }

  return prev;
}

function appendMarker(prev: Message[], ev: ServerEvent, text: string, ts: number): Message[] {
  const id = ev.meta.id || `mk-${ts}-${prev.length}`;
  // 防重：相同 id 不再 push
  if (prev.some((m) => m.id === id)) return prev;
  const next: StatusMarker = { kind: "marker", id, text, ts };
  return [...prev, next];
}

function stringifyArgs(input: unknown): string | null {
  if (input == null) return null;
  if (typeof input === "string") return input;
  try {
    const s = JSON.stringify(input);
    if (!s) return null;
    return s.length > 80 ? `${s.slice(0, 77)}...` : s;
  } catch {
    return null;
  }
}

function stringifyOutput(output: unknown): string | null {
  if (output == null) return null;
  if (typeof output === "string") return output;
  try {
    return JSON.stringify(output, null, 2);
  } catch {
    return null;
  }
}

// ============================================================================
// 任务 thread (#39 / #N4') 专用 reducer · `applyTaskThreadEvent`
// ----------------------------------------------------------------------------
// 跟 applyWorkerEvent 的差异：
//   - 多 agent 上下文：玄女 + 各 worker 都进同一 thread（按时间）
//   - 玄女 (role=xuannv) 走 XuannvMessage（紫色 who）
//   - worker 走 WorkerMessage（角色色 who）
//   - UserInterventionSent → user bubble 含 mentions（chip 还原），不管 target 是谁
//   - tool_call/tool_result/thinking 配 agent，不强制单 agent 限制
// 服务端 /api/tasks/:id/conv 已 filter by meta.task + 白名单 EventKind，前端只需做渲染映射。
// ============================================================================

export interface TaskThreadCtx {
  /** 该任务的 agent_id → { role, role_display } 映射，用于 bubble 着色 + who 标签。
   *  member lookup 来自 TaskGroupCard.members；玄女不在 members 里时降级用 role_display="玄女"。*/
  members: Record<string, { role: string; role_display: string }>;
  /** 玄女的 agent_id，用于 user_intervention_sent target 等于它时识别"对玄女说"。
   *  v1 简化：玄女 role = "xuannv" 也作识别，无需 xuannv_id 必填。 */
  xuannv_id?: string;
}

function lookupMember(ctx: TaskThreadCtx, agent: string | null | undefined):
  | { role: string; role_display: string }
  | null {
  if (!agent) return null;
  return ctx.members[agent] ?? null;
}

function isXuannvAgent(ctx: TaskThreadCtx, agent: string | null | undefined): boolean {
  if (!agent) return false;
  if (agent === ctx.xuannv_id) return true;
  const m = lookupMember(ctx, agent);
  return m?.role === "xuannv";
}

function lastStreamingFrom(prev: Message[], agent: string): WorkerMessage | XuannvMessage | null {
  for (let i = prev.length - 1; i >= 0; i -= 1) {
    const m = prev[i];
    if (!m) continue;
    if (m.kind === "worker" && m.agent === agent && m.streaming) return m;
    if (m.kind === "xuannv" && m.agent === agent && m.streaming) return m;
    // 一旦遇到非 streaming 的同 agent 或不同 agent 终态，就停止往前找
    if (m.kind === "worker" || m.kind === "xuannv") return null;
  }
  return null;
}

function lastStreamingThinkingFrom(prev: Message[], agent: string): ThinkingMessage | null {
  for (let i = prev.length - 1; i >= 0; i -= 1) {
    const m = prev[i];
    if (!m) continue;
    if (m.kind === "thinking" && m.agent === agent && m.streaming) return m;
  }
  return null;
}

function startTaskBubble(
  prev: Message[],
  ev: ServerEvent,
  ctx: TaskThreadCtx,
  initialText: string,
): Message[] {
  const ts = parseTs(ev.meta.at);
  const agent = ev.meta.agent ?? "";
  const id = ev.meta.id || `m-${ts}-${prev.length}`;
  if (isXuannvAgent(ctx, agent)) {
    const next: XuannvMessage = {
      kind: "xuannv",
      id,
      agent,
      text: initialText,
      streaming: true,
      ts,
    };
    return [...prev, next];
  }
  const member = lookupMember(ctx, agent);
  const next: WorkerMessage = {
    kind: "worker",
    id,
    agent,
    role: member?.role,
    role_display: member?.role_display ?? "门客",
    text: initialText,
    streaming: true,
    ts,
  };
  return [...prev, next];
}

export function applyTaskThreadEvent(
  prev: Message[],
  ev: ServerEvent,
  ctx: TaskThreadCtx,
): Message[] {
  if (!ev || !ev.kind || typeof ev.kind.type !== "string") return prev;
  const k = ev.kind;
  const ts = parseTs(ev.meta.at);
  const agent = ev.meta.agent ?? "";

  // —— 用户 intervene · 渲染右侧 user bubble + 还原 chip（mentions）——
  if (k.type === "user_intervention_sent") {
    const text = (k as { text?: string }).text ?? "";
    if (text.trim() === "") return prev;
    const id = ev.meta.id || `u-${ts}-${prev.length}`;
    if (prev.some((m) => m.id === id)) return prev;
    const mentions = (k as { mentions?: string[] }).mentions ?? [];
    const next: UserMessage = {
      kind: "user",
      id,
      text,
      mentions: mentions.length > 0 ? mentions : undefined,
      pending: false,
      ts,
    };
    return [...prev, next];
  }

  // —— thinking 折叠条 · 起 / 完结 ——
  if (k.type === "thinking_started") {
    if (!agent) return prev;
    if (lastStreamingThinkingFrom(prev, agent)) return prev;
    const next: ThinkingMessage = {
      kind: "thinking",
      id: ev.meta.id || `th-${ts}-${prev.length}`,
      agent,
      streaming: true,
      ts,
    };
    return [...prev, next];
  }
  if (k.type === "thinking_finished") {
    if (!agent) return prev;
    const last = lastStreamingThinkingFrom(prev, agent);
    if (!last) return prev;
    const dur = ts - last.ts;
    const updated: ThinkingMessage = {
      ...last,
      streaming: false,
      duration_ms: dur >= 0 ? dur : null,
    };
    const idx = prev.indexOf(last);
    const next = prev.slice();
    next[idx] = updated;
    return next;
  }

  // —— 文本 delta / 整段 ——
  if (k.type === "agent_text_delta") {
    if (!agent) return prev;
    const delta = (k as { delta?: string }).delta ?? "";
    const last = lastStreamingFrom(prev, agent);
    if (last) {
      const updated = { ...last, text: last.text + delta };
      const idx = prev.indexOf(last);
      const next = prev.slice();
      next[idx] = updated;
      return next;
    }
    return startTaskBubble(prev, ev, ctx, delta);
  }
  if (k.type === "agent_text" || k.type === "agent_responded") {
    if (!agent) return prev;
    const text = (k as { text?: string }).text ?? "";
    const isEmpty = text.trim() === "";
    const last = lastStreamingFrom(prev, agent);
    if (last) {
      if (isEmpty) {
        // 空 turn：丢空 bubble（Bug #24 同款防御）
        return prev.filter((m) => m !== last);
      }
      const updated = { ...last, text, streaming: false };
      const idx = prev.indexOf(last);
      const next = prev.slice();
      next[idx] = updated;
      return next;
    }
    if (isEmpty) return prev;
    if (isXuannvAgent(ctx, agent)) {
      const next: XuannvMessage = {
        kind: "xuannv",
        id: ev.meta.id || `xn-${ts}-${prev.length}`,
        agent,
        text,
        streaming: false,
        ts,
      };
      return [...prev, next];
    }
    const member = lookupMember(ctx, agent);
    const next: WorkerMessage = {
      kind: "worker",
      id: ev.meta.id || `w-${ts}-${prev.length}`,
      agent,
      role: member?.role,
      role_display: member?.role_display ?? "门客",
      text,
      streaming: false,
      ts,
    };
    return [...prev, next];
  }

  // —— 工具调用配对 ——
  if (k.type === "tool_call") {
    if (!agent) return prev;
    const tool = (k as { tool?: string }).tool ?? "tool";
    const args = stringifyArgs((k as { input?: unknown }).input);
    const next: ToolCallMessage = {
      kind: "tool_call",
      id: ev.meta.id || `tc-${ts}-${prev.length}`,
      agent,
      tool,
      args_summary: args,
      status: "running",
      ts,
    };
    return [...prev, next];
  }
  if (k.type === "tool_result") {
    if (!agent) return prev;
    const tool = (k as { tool?: string }).tool ?? "tool";
    const ok = Boolean((k as { ok?: boolean }).ok);
    const output = stringifyOutput((k as { output?: unknown }).output);
    const idx = findRunningToolIdx(prev, agent, tool);
    if (idx >= 0) {
      const running = prev[idx] as ToolCallMessage;
      const updated: ToolCallMessage = {
        ...running,
        status: ok ? "ok" : "err",
        duration_ms: ts - running.ts,
        output,
      };
      const next = prev.slice();
      next[idx] = updated;
      return next;
    }
    // 未配上 started · 直接插完结态卡
    const next: ToolCallMessage = {
      kind: "tool_call",
      id: ev.meta.id || `tc-${ts}-${prev.length}`,
      agent,
      tool,
      status: ok ? "ok" : "err",
      duration_ms: 0,
      output,
      ts,
    };
    return [...prev, next];
  }

  // —— 状态变更 marker ——
  if (k.type === "agent_idle") {
    const member = lookupMember(ctx, agent);
    const text = `${member?.role_display ?? "门客"} idle`;
    return appendMarker(prev, ev, text, ts);
  }
  if (k.type === "task_completed") {
    const summary = (k as { summary?: string }).summary;
    const text = summary ? `任务完成 · ${summary}` : "任务完成";
    return appendMarker(prev, ev, text, ts);
  }
  if (k.type === "task_state_changed") {
    const to = (k as { to?: string }).to ?? "";
    if (to === "Done" || to === "done") return appendMarker(prev, ev, "任务完成", ts);
    if (to === "Cancelled" || to === "cancelled") return appendMarker(prev, ev, "已取消", ts);
    if (to === "Failed" || to === "failed") return appendMarker(prev, ev, "任务失败", ts);
    return prev;
  }

  return prev;
}
