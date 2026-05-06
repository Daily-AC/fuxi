// @ 提及逻辑 · v3 #N2' / #37 + dist #60
//
// 设计 spec:
//   - docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §C / §"composer @ 机制详细规范"
//   - docs/superpowers/specs/2026-04-27-im-dist-接通-design.md §"PWA composer @ pinned-node 解析"
//
// 候选两种 kind：worker（默认 · 现有）和 node（dist topology 节点 · #60）。
// 序列化时 worker chips → mentions[]；node chips → pinned_node（取第一个）。

import type { NodeView, ProjectView, TaskMember } from "~/types/api";

/** 候选的 kind：worker = 现有 agent；node = dist topology 节点（#60）；
 *  project = 已注册项目（v2 跨节点 sandbox · session14+15）。
 *  缺省视为 worker（旧调用兼容）。
 *
 *  序列化路径：
 *  - worker chip → mentions[]（target = mentions[0]）
 *  - node chip → pinned_node（取第一个）
 *  - project chip → project（取第一个；多于一个 multi_project=true）
 *
 *  project chip 跟 worker / node 互斥语义：composer 序列化看到 project 时
 *  上层应当走 `client.dispatch`（创新 task）而不是 intervene。具体派活逻辑由
 *  caller（XuannvPage handleSubmit）按 SerializedIntervene.project 是否 set 决定。*/
export type MentionKind = "worker" | "node" | "project";

export interface MentionCandidate {
  /** worker 用 agent_id；node 用 node_id。chip 上一律走 id。*/
  agent_id: string;
  /** worker：role key（"luban" / "xuannv" / ...）；node：固定 "node"（着色查蓝色专属色）。*/
  role: string;
  /** 显示名："鲁班" / "蒲松" / "玄女" / node_id ("home" / "mac-local")。*/
  role_display: string;
  /** autocomplete 副文本：last_tool_call.tool / 节点 inflight/online 状态。*/
  hint?: string | null;
  /** 排序用（last_active_at ISO）。null/missing → 排底。*/
  last_active_at?: string | null;
  /** #60 加 · 候选种类。缺省 worker 兼容老调用。 */
  kind?: MentionKind;
}

/** Composer 内的 chip token · 序列化时按 spec §"发送时序列化" 用零宽 + agent_id 占位。
 *  chip 在 composer 是不可分割单元（光标只能在前后），结构上保持成 [text, chip, text, chip, ...] 段。
 *  #60：chip 加 kind 区分 worker/node；node chip 走 pinned_node 路径不走 mentions。 */
export interface MentionChipToken {
  agent_id: string;
  role: string;
  role_display: string;
  /** #60 加 · 缺省 worker。node chip 序列化为 pinned_node。*/
  kind?: MentionKind;
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
    kind: "worker" as MentionKind,
  }));
}

/** #60：dist NodeView 数组转 MentionCandidate · 注入 composer @ 候选尾部。
 *  - 仅 online 节点入候选（用户 @ 离线节点没意义）
 *  - hint：inflight/max_concurrency 直观显容量（"2/4 个任务"）
 *  - role 固定 "node"，给 colorForRole + chip 走蓝色专属色 #7AA0E5（spec §gap e）*/
export function candidatesFromNodes(nodes: NodeView[]): MentionCandidate[] {
  return nodes
    .filter((n) => n.online)
    .map((n) => ({
      agent_id: n.node_id,
      role: "node",
      role_display: n.node_id,
      hint: `${n.inflight_jobs}/${n.max_concurrency} 个任务`,
      last_active_at: null,
      kind: "node" as MentionKind,
    }));
}

/** v2 跨节点 sandbox：ProjectView 数组转 MentionCandidate。
 *  - agent_id 用 project.id (slug)；上层序列化 → SerializedIntervene.project
 *  - role 固定 "project"，chip 走绿色专属色（区分 worker 黄 + node 蓝）
 *  - hint 显 default_branch（让用户一眼知道往哪条 base 派）*/
export function candidatesFromProjects(projects: ProjectView[]): MentionCandidate[] {
  return projects.map((p) => ({
    agent_id: p.id,
    role: "project",
    role_display: p.id,
    hint: p.default_branch,
    last_active_at: null,
    kind: "project" as MentionKind,
  }));
}

function hintForMember(m: TaskMember): string | null {
  if (m.last_activity) return m.last_activity;
  const tool = toolCallText(m.last_tool_call);
  if (tool) return tool;
  if (m.activity) return m.activity;
  if (m.status === "idle") return "待命";
  if (m.status === "thinking") return "思考中";
  return "运行中";
}

function toolCallText(call: TaskMember["last_tool_call"]): string | null {
  if (!call) return null;
  if (typeof call === "string") return call;
  return call.args_summary ? `${call.tool} ${call.args_summary}` : call.tool;
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

/** Intervene 请求 · target 取第一个 worker chip；无 worker chip 时 undefined（backend 用玄女默认）。
 *  #60：node chip 走 pinned_node 字段，跟 worker mentions 互不干扰。
 *  v2 跨节点：project chip 走 project 字段，caller 检测 project set → 走 dispatch 路径。 */
export interface SerializedIntervene {
  /** target = 第一个 worker chip 的 agent_id；无 worker chip 时 undefined（backend 默认走玄女）。 */
  target?: string;
  /** 文本部分（chip 占位用零宽间隔符 ​）。*/
  text: string;
  /** 所有 worker chip 的 agent_id（按出现顺序）。 */
  mentions: string[];
  /** worker chip 数 > 1 时 true，UI 用来 toast 警示「只发给第一个」。 */
  multi: boolean;
  /** v3 #46 加 · 已上传附件的 upload id 列表（β #17 attachments 字段）。 */
  attachments?: string[];
  /** #60 加 · 第一个 node chip 的 node_id（无 node chip 时 undefined）。
   *  契约：β #57 后端 InterveneRequest 加 pinned_node 解析 → Fuxi::dispatch 走 dist enqueue。*/
  pinned_node?: string;
  /** #60 加 · node chip 数 > 1 时 true，UI 用来 toast 警示「多于一个节点 chip 取第一个」。 */
  multi_node: boolean;
  /** v2 跨节点：第一个 project chip 的 slug（无 project chip 时 undefined）。
   *  契约：caller 见 project set → 调 `client.dispatch({ project, ... })` 创新 task；
   *  不 set 时走原 intervene 路径。auto-pin 由后端按 project.host_nodes 选最闲节点。*/
  project?: string;
  /** project chip 数 > 1 时 true，UI 用来 toast 警示「只派给第一个项目」。 */
  multi_project: boolean;
}

const CHIP_PLACEHOLDER = "​";

/** segments → intervene 请求 body。
 *  - worker chip → mentions[]（target = mentions[0]）
 *  - node chip → pinned_node（取第一个；多于一个 multi_node=true → UI toast 警示）
 *  - chip 占位用零宽间隔符 ​（U+200B）保留位置但不显字符
 *
 *  fallbackAgentId 可选：当前 v3 玄女 tab 不需要传（无 chip 时让 backend 默认走玄女）。 */
export function serializeComposer(
  segments: ComposerSegment[],
  fallbackAgentId?: string,
): SerializedIntervene {
  const mentions: string[] = [];
  const nodeIds: string[] = [];
  const projectSlugs: string[] = [];
  let text = "";
  for (const seg of segments) {
    if (seg.kind === "text") {
      text += seg.text;
      continue;
    }
    if (seg.chip.kind === "node") {
      nodeIds.push(seg.chip.agent_id);
    } else if (seg.chip.kind === "project") {
      projectSlugs.push(seg.chip.agent_id);
    } else {
      mentions.push(seg.chip.agent_id);
    }
    text += CHIP_PLACEHOLDER;
  }
  const target = mentions[0] ?? fallbackAgentId;
  const pinned_node = nodeIds[0];
  const project = projectSlugs[0];
  return {
    target,
    text: text.trim(),
    mentions,
    multi: mentions.length > 1,
    pinned_node,
    multi_node: nodeIds.length > 1,
    project,
    multi_project: projectSlugs.length > 1,
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

/** #60：多于一个 node chip 时 toast 文案（spec §gap e）。*/
export const MULTI_NODE_WARNING =
  "fuxi 当前只 pin 第一个 @ 的节点，其余仅作引用";

/** #60：node chip 专属蓝色（spec §gap e 钉的 #7AA0E5）。
 *  独立 export 让 MentionChip / NodeChip 共用同一颜色源。*/
export const NODE_CHIP_COLOR = "#7AA0E5";

/** v2 跨节点：多于一个 project chip 时 toast 文案。 */
export const MULTI_PROJECT_WARNING =
  "fuxi 当前只派给第一个 @ 的项目，其余仅作引用";

/** v2 跨节点：project chip 专属绿色（区分 worker 黄 + node 蓝）。 */
export const PROJECT_CHIP_COLOR = "#7AC97A";
