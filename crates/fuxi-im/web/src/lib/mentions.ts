// @ 提及逻辑 · v3 #N2' / #37
//
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §C / §"composer @ 机制详细规范"
//
// 三块：
//   1. MentionCandidate 类型 + 排序（last_active_at 降序）
//   2. fuzzyMatch · 拼音/汉字/role 名 match 候选
//   3. 序列化：composer 文本 + chip 数组 → { target, text, mentions } intervene 请求

import type { TaskMember } from "~/types/api";

export interface MentionCandidate {
  agent_id: string;
  /** role key，"luban" / "pusong" / "xuannv" / ...。用于 colorForRole + role_color 选择。*/
  role: string;
  /** 显示名："鲁班" / "蒲松" / "玄女"。*/
  role_display: string;
  /** autocomplete 副文本：last_tool_call.tool 或 status fallback ("待命" / "思考中" / ...)。*/
  hint?: string | null;
  /** 排序用（last_active_at ISO）。null/missing → 排底。*/
  last_active_at?: string | null;
}

/** Composer 内的 chip token · 序列化时按 spec §"发送时序列化" 用零宽 + agent_id 占位。
 *  chip 在 composer 是不可分割单元（光标只能在前后），结构上保持成 [text, chip, text, chip, ...] 段。*/
export interface MentionChipToken {
  agent_id: string;
  role: string;
  role_display: string;
}

/** Composer state 段落组：text 段和 chip 段交错。
 *  例：用户输入 "查 ERP @鲁班 的代码" 选中鲁班后变成
 *  [{ kind: "text", text: "查 ERP " }, { kind: "chip", chip: {luban} }, { kind: "text", text: " 的代码" }]
 */
export type ComposerSegment =
  | { kind: "text"; text: string }
  | { kind: "chip"; chip: MentionChipToken };

/** 把 TaskMember 数组转 MentionCandidate · 任务 thread 用。 */
export function candidatesFromMembers(
  members: TaskMember[],
): MentionCandidate[] {
  return members.map((m) => ({
    agent_id: m.agent_id,
    role: m.role,
    role_display: m.role_display,
    hint: hintForMember(m),
    last_active_at: null,
  }));
}

function hintForMember(m: TaskMember): string | null {
  const tool = m.last_tool_call?.tool;
  if (tool) {
    const args = m.last_tool_call?.args_summary;
    return args ? `${tool} ${args}` : tool;
  }
  if (m.activity) return m.activity;
  if (m.status === "idle") return "待命";
  if (m.status === "thinking") return "思考中";
  return "运行中";
}

/** 候选排序：last_active_at 降序，缺失值排底。原数组不变。*/
export function sortCandidates(list: MentionCandidate[]): MentionCandidate[] {
  return list.slice().sort((a, b) => {
    const ta = a.last_active_at ? Date.parse(a.last_active_at) : 0;
    const tb = b.last_active_at ? Date.parse(b.last_active_at) : 0;
    if (ta !== tb) return tb - ta;
    return 0;
  });
}

/** fuzzy match · 简化策略：query 命中 role_display（中文）或 role（英文 key）的子串。
 *  v1 不接拼音输入法（中文 IME 不触发 @，已在 spec §触发段）；只做 substring + 大小写不敏感。 */
export function fuzzyMatch(list: MentionCandidate[], query: string): MentionCandidate[] {
  const q = query.trim().toLowerCase();
  if (q === "") return list;
  return list.filter((c) => {
    const roleDisplay = (c.role_display ?? "").toLowerCase();
    const roleKey = (c.role ?? "").toLowerCase();
    return roleDisplay.includes(q) || roleKey.includes(q);
  });
}

/** Intervene 请求 · target 取第一个 chip 的 agent_id；无 chip 时 undefined（让 backend 用玄女默认）。*/
export interface SerializedIntervene {
  /** target = mentions[0]；无 chip 时 undefined（backend 默认走玄女）。 */
  target?: string;
  /** 文本部分（chip 占位用零宽间隔符 ​）。*/
  text: string;
  /** 所有 chip 的 agent_id（按出现顺序）。*/
  mentions: string[];
  /** chip 数 > 1 时设 true，UI 用来 toast 警示「只发给第一个」。*/
  multi: boolean;
}

const CHIP_PLACEHOLDER = "​";

/** segments → intervene 请求 body（含 mentions）。
 *  fallbackAgentId 可选：当前 v3 玄女 tab 不需要传（无 chip 时让 backend 默认走玄女）。
 *  任务 thread 内可传任务发起人 agent_id 作 fallback target —— spec §D 任务 thread 默认对玄女说，
 *  跟玄女 tab 默认行为一致，所以 fallback 也可省。*/
export function serializeComposer(
  segments: ComposerSegment[],
  fallbackAgentId?: string,
): SerializedIntervene {
  const mentions: string[] = [];
  let text = "";
  for (const seg of segments) {
    if (seg.kind === "text") {
      text += seg.text;
    } else {
      mentions.push(seg.chip.agent_id);
      text += CHIP_PLACEHOLDER;
    }
  }
  const target = mentions[0] ?? fallbackAgentId;
  return {
    target,
    text: text.trim(),
    mentions,
    multi: mentions.length > 1,
  };
}

/** 从 segments 提取所有 chip · 用于 v1.x 渲染或重组。*/
export function chipsOf(segments: ComposerSegment[]): MentionChipToken[] {
  const out: MentionChipToken[] = [];
  for (const seg of segments) {
    if (seg.kind === "chip") out.push(seg.chip);
  }
  return out;
}

/** 重新拼合所有 text 段，用于纯文本预览（chip 替换为 @role_display）。
 *  历史消息渲染用：后端把 mentions 一起回来，前端按出现顺序还原 chip。*/
export function previewText(segments: ComposerSegment[]): string {
  return segments
    .map((s) => (s.kind === "text" ? s.text : `@${s.chip.role_display}`))
    .join("");
}

/** v1 多 chip toast 文案（spec §多 @ 处理）。*/
export const MULTI_MENTION_WARNING =
  "fuxi 当前只发给第一个 @ 的角色，其余仅作引用";
