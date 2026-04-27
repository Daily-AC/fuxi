import { describe, expect, it } from "vitest";
import {
  candidatesFromMembers,
  candidatesFromNodes,
  chipsOf,
  fuzzyMatch,
  previewText,
  serializeComposer,
  sortCandidates,
  MULTI_MENTION_WARNING,
  MULTI_NODE_WARNING,
  NODE_CHIP_COLOR,
  type ComposerSegment,
  type MentionCandidate,
} from "~/lib/mentions";
import type { NodeView, TaskMember } from "~/types/api";

const LUBAN: MentionCandidate = {
  agent_id: "a-luban",
  role: "luban",
  role_display: "鲁班",
  hint: "grep server",
  last_active_at: "2026-04-26T12:00:00Z",
};
const PUSONG: MentionCandidate = {
  agent_id: "a-pusong",
  role: "pusong",
  role_display: "蒲松",
  hint: "待命",
  last_active_at: "2026-04-26T11:00:00Z",
};
const XUANNV: MentionCandidate = {
  agent_id: "a-xuannv",
  role: "xuannv",
  role_display: "玄女",
  hint: null,
  last_active_at: null,
};

describe("mentions · candidatesFromMembers", () => {
  it("把 TaskMember 转 MentionCandidate · last_tool_call.tool 优先", () => {
    const members: TaskMember[] = [
      {
        agent_id: "a-luban",
        role: "luban",
        role_display: "鲁班",
        status: "busy",
        last_tool_call: { tool: "Bash", args_summary: "grep server" },
      },
    ];
    const out = candidatesFromMembers(members);
    expect(out[0]?.hint).toBe("Bash grep server");
  });

  it("activity fallback / status 兜底", () => {
    const members: TaskMember[] = [
      {
        agent_id: "a-1",
        role: "luban",
        role_display: "鲁班",
        status: "idle",
      },
      {
        agent_id: "a-2",
        role: "pusong",
        role_display: "蒲松",
        status: "thinking",
        activity: "Read git log",
      },
    ];
    const out = candidatesFromMembers(members);
    expect(out[0]?.hint).toBe("待命");
    expect(out[1]?.hint).toBe("Read git log");
  });
});

describe("mentions · sortCandidates", () => {
  it("按 last_active_at 降序，缺失值排底", () => {
    const out = sortCandidates([XUANNV, PUSONG, LUBAN]);
    expect(out.map((c) => c.role)).toEqual(["luban", "pusong", "xuannv"]);
  });

  it("纯函数 · 不改原数组", () => {
    const input = [XUANNV, PUSONG, LUBAN];
    sortCandidates(input);
    expect(input.map((c) => c.role)).toEqual(["xuannv", "pusong", "luban"]);
  });
});

describe("mentions · fuzzyMatch", () => {
  const list = [LUBAN, PUSONG, XUANNV];

  it("空 query · 全返回", () => {
    expect(fuzzyMatch(list, "")).toEqual(list);
    expect(fuzzyMatch(list, "  ")).toEqual(list);
  });

  it("中文子串匹配", () => {
    expect(fuzzyMatch(list, "鲁").map((c) => c.role)).toEqual(["luban"]);
    expect(fuzzyMatch(list, "玄").map((c) => c.role)).toEqual(["xuannv"]);
  });

  it("英文 role key 匹配 · 大小写不敏感", () => {
    expect(fuzzyMatch(list, "Lu").map((c) => c.role)).toEqual(["luban"]);
    expect(fuzzyMatch(list, "PUSONG").map((c) => c.role)).toEqual(["pusong"]);
  });

  it("没匹配 · 空数组", () => {
    expect(fuzzyMatch(list, "孔子")).toEqual([]);
  });
});

describe("mentions · serializeComposer", () => {
  it("无 chip · target = fallback，mentions 为空", () => {
    const segs: ComposerSegment[] = [{ kind: "text", text: "你好玄女" }];
    const out = serializeComposer(segs, "x-uuid");
    expect(out.target).toBe("x-uuid");
    expect(out.text).toBe("你好玄女");
    expect(out.mentions).toEqual([]);
    expect(out.multi).toBe(false);
  });

  it("无 chip + 无 fallback · target=undefined（backend 走玄女默认）", () => {
    const segs: ComposerSegment[] = [{ kind: "text", text: "嗨玄女" }];
    const out = serializeComposer(segs);
    expect(out.target).toBeUndefined();
    expect(out.mentions).toEqual([]);
    expect(out.text).toBe("嗨玄女");
  });

  it("一个 chip · target = chip.agent_id", () => {
    const segs: ComposerSegment[] = [
      { kind: "text", text: "查 ERP " },
      {
        kind: "chip",
        chip: { agent_id: "a-luban", role: "luban", role_display: "鲁班" },
      },
      { kind: "text", text: " 接口" },
    ];
    const out = serializeComposer(segs, "x-uuid");
    expect(out.target).toBe("a-luban");
    expect(out.mentions).toEqual(["a-luban"]);
    expect(out.multi).toBe(false);
    // 文本含 placeholder 零宽字符
    expect(out.text).toContain("ERP");
    expect(out.text).toContain("接口");
  });

  it("两个 chip · target = mentions[0]，multi=true", () => {
    const segs: ComposerSegment[] = [
      {
        kind: "chip",
        chip: { agent_id: "a-luban", role: "luban", role_display: "鲁班" },
      },
      { kind: "text", text: " " },
      {
        kind: "chip",
        chip: { agent_id: "a-pusong", role: "pusong", role_display: "蒲松" },
      },
      { kind: "text", text: " 一起干" },
    ];
    const out = serializeComposer(segs, "x-uuid");
    expect(out.target).toBe("a-luban");
    expect(out.mentions).toEqual(["a-luban", "a-pusong"]);
    expect(out.multi).toBe(true);
  });

  it("MULTI_MENTION_WARNING 文案匹配 spec", () => {
    expect(MULTI_MENTION_WARNING).toContain("第一个");
    expect(MULTI_MENTION_WARNING).toContain("引用");
  });
});

describe("mentions · #60 dist node 候选 + pinned_node", () => {
  const NODES: NodeView[] = [
    {
      node_id: "home",
      tags: ["home"],
      max_concurrency: 4,
      inflight_jobs: 1,
      heartbeat_lag_ms: 100,
      online: true,
      registered_at: null,
      workers: [],
    },
    {
      node_id: "mac-local",
      tags: ["local", "mac"],
      max_concurrency: 8,
      inflight_jobs: 0,
      heartbeat_lag_ms: 800,
      online: true,
      registered_at: null,
      workers: [],
    },
    {
      node_id: "offline-node",
      tags: [],
      max_concurrency: 4,
      inflight_jobs: 0,
      heartbeat_lag_ms: 99999,
      online: false,
      registered_at: null,
      workers: [],
    },
  ];

  it("candidatesFromNodes · 仅 online 节点 + role='node' + kind='node' + hint inflight/cap", () => {
    const out = candidatesFromNodes(NODES);
    expect(out.length).toBe(2);
    expect(out.map((c) => c.agent_id)).toEqual(["home", "mac-local"]);
    expect(out.every((c) => c.kind === "node")).toBe(true);
    expect(out.every((c) => c.role === "node")).toBe(true);
    expect(out[0]?.hint).toBe("1/4 个任务");
    expect(out[1]?.hint).toBe("0/8 个任务");
  });

  it("serializeComposer · 单 node chip · pinned_node = node_id · multi_node=false", () => {
    const segs: ComposerSegment[] = [
      { kind: "text", text: "用 " },
      {
        kind: "chip",
        chip: { agent_id: "mac-local", role: "node", role_display: "mac-local", kind: "node" },
      },
      { kind: "text", text: " 跑" },
    ];
    const out = serializeComposer(segs);
    expect(out.pinned_node).toBe("mac-local");
    expect(out.mentions).toEqual([]);
    expect(out.target).toBeUndefined();
    expect(out.multi_node).toBe(false);
  });

  it("serializeComposer · worker chip + node chip mix · 各走各路", () => {
    const segs: ComposerSegment[] = [
      {
        kind: "chip",
        chip: { agent_id: "a-luban", role: "luban", role_display: "鲁班", kind: "worker" },
      },
      { kind: "text", text: " 跑 " },
      {
        kind: "chip",
        chip: { agent_id: "mac-local", role: "node", role_display: "mac-local", kind: "node" },
      },
    ];
    const out = serializeComposer(segs);
    expect(out.target).toBe("a-luban");
    expect(out.mentions).toEqual(["a-luban"]);
    expect(out.pinned_node).toBe("mac-local");
    expect(out.multi).toBe(false);
    expect(out.multi_node).toBe(false);
  });

  it("serializeComposer · 两个 node chip · multi_node=true · 取第一个", () => {
    const segs: ComposerSegment[] = [
      {
        kind: "chip",
        chip: { agent_id: "home", role: "node", role_display: "home", kind: "node" },
      },
      { kind: "text", text: " " },
      {
        kind: "chip",
        chip: { agent_id: "mac-local", role: "node", role_display: "mac-local", kind: "node" },
      },
    ];
    const out = serializeComposer(segs);
    expect(out.pinned_node).toBe("home");
    expect(out.multi_node).toBe(true);
  });

  it("无 node chip · pinned_node 为 undefined（不发字段）", () => {
    const segs: ComposerSegment[] = [{ kind: "text", text: "你好" }];
    const out = serializeComposer(segs);
    expect(out.pinned_node).toBeUndefined();
    expect(out.multi_node).toBe(false);
  });

  it("MULTI_NODE_WARNING 文案匹配 spec（多于一个节点 chip 取第一个）", () => {
    expect(MULTI_NODE_WARNING).toContain("第一个");
    expect(MULTI_NODE_WARNING).toContain("节点");
  });

  it("NODE_CHIP_COLOR · spec 钉的 #7AA0E5 蓝色", () => {
    expect(NODE_CHIP_COLOR.toLowerCase()).toBe("#7aa0e5");
  });

  it("缺省 chip.kind · 视为 worker（兼容老 chip token）", () => {
    const segs: ComposerSegment[] = [
      {
        kind: "chip",
        chip: { agent_id: "a-luban", role: "luban", role_display: "鲁班" },
      },
    ];
    const out = serializeComposer(segs);
    expect(out.mentions).toEqual(["a-luban"]);
    expect(out.pinned_node).toBeUndefined();
  });
});

describe("mentions · chipsOf / previewText", () => {
  const segs: ComposerSegment[] = [
    { kind: "text", text: "嘿 " },
    {
      kind: "chip",
      chip: { agent_id: "a-luban", role: "luban", role_display: "鲁班" },
    },
    { kind: "text", text: " 干活" },
  ];

  it("chipsOf 提取所有 chip", () => {
    const out = chipsOf(segs);
    expect(out.length).toBe(1);
    expect(out[0]?.agent_id).toBe("a-luban");
  });

  it("previewText 还原成 @role_display 文本", () => {
    expect(previewText(segs)).toBe("嘿 @鲁班 干活");
  });
});
