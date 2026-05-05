import { describe, expect, it } from "vitest";
import {
  groupConsecutiveToolCalls,
  type Message,
  type ToolCallMessage,
  type ToolGroupView,
} from "~/messages";

const LUBAN = "agent-luban-uuid";
const XUANNV = "agent-xuannv-uuid";

function tool(id: string, agent: string, ts: number = 1): ToolCallMessage {
  return {
    kind: "tool_call",
    id,
    agent,
    tool: "Bash",
    args_summary: `cmd-${id}`,
    status: "ok",
    duration_ms: 100,
    output: null,
    ts,
  };
}

function worker(id: string, agent: string, text: string): Message {
  return {
    kind: "worker",
    id,
    agent,
    role_display: "鲁班",
    text,
    streaming: false,
    ts: 0,
  };
}

describe("groupConsecutiveToolCalls", () => {
  it("空数组 → 空", () => {
    expect(groupConsecutiveToolCalls([])).toEqual([]);
  });

  it("无 tool_call → 原数组直传", () => {
    const w1 = worker("w1", LUBAN, "hi");
    const w2 = worker("w2", LUBAN, "ok");
    expect(groupConsecutiveToolCalls([w1, w2])).toEqual([w1, w2]);
  });

  it("单条 tool_call → 不折叠", () => {
    const t1 = tool("t1", LUBAN);
    const out = groupConsecutiveToolCalls([t1]);
    expect(out).toHaveLength(1);
    expect(out[0]?.kind).toBe("tool_call");
  });

  it("连续 ≥2 同 agent tool_call → 合一组", () => {
    const t1 = tool("t1", LUBAN, 1);
    const t2 = tool("t2", LUBAN, 2);
    const t3 = tool("t3", LUBAN, 3);
    const out = groupConsecutiveToolCalls([t1, t2, t3]);
    expect(out).toHaveLength(1);
    const g = out[0] as ToolGroupView;
    expect(g.kind).toBe("tool_group");
    expect(g.items).toHaveLength(3);
    expect(g.agent).toBe(LUBAN);
    expect(g.id).toBe("tg-t1"); // 第一条 ev id
    expect(g.ts).toBe(1);
  });

  it("中间夹 worker 文本 → 切两组", () => {
    const t1 = tool("t1", LUBAN);
    const t2 = tool("t2", LUBAN);
    const w1 = worker("w1", LUBAN, "中间评论");
    const t3 = tool("t3", LUBAN);
    const t4 = tool("t4", LUBAN);
    const out = groupConsecutiveToolCalls([t1, t2, w1, t3, t4]);
    expect(out).toHaveLength(3); // group1 + worker + group2
    expect(out[0]?.kind).toBe("tool_group");
    expect((out[0] as ToolGroupView).items).toHaveLength(2);
    expect(out[1]?.kind).toBe("worker");
    expect(out[2]?.kind).toBe("tool_group");
    expect((out[2] as ToolGroupView).items).toHaveLength(2);
  });

  it("不同 agent 切组（即便连续）", () => {
    const a = tool("a", LUBAN);
    const b = tool("b", LUBAN);
    const c = tool("c", XUANNV);
    const d = tool("d", XUANNV);
    const out = groupConsecutiveToolCalls([a, b, c, d]);
    expect(out).toHaveLength(2);
    expect((out[0] as ToolGroupView).agent).toBe(LUBAN);
    expect((out[1] as ToolGroupView).agent).toBe(XUANNV);
  });

  it("单条 + 连续 + 单条 → 单条不折叠 / 连续折叠", () => {
    const t1 = tool("t1", LUBAN);
    const w1 = worker("w1", LUBAN, "x");
    const t2 = tool("t2", LUBAN);
    const t3 = tool("t3", LUBAN);
    const w2 = worker("w2", LUBAN, "y");
    const t4 = tool("t4", LUBAN);
    const out = groupConsecutiveToolCalls([t1, w1, t2, t3, w2, t4]);
    expect(out).toHaveLength(5); // tool / worker / group(2) / worker / tool
    expect(out[0]?.kind).toBe("tool_call");
    expect(out[1]?.kind).toBe("worker");
    expect(out[2]?.kind).toBe("tool_group");
    expect((out[2] as ToolGroupView).items).toHaveLength(2);
    expect(out[3]?.kind).toBe("worker");
    expect(out[4]?.kind).toBe("tool_call");
  });

  it("agent id 前缀差异（agent-luban vs luban）走 eqAgent 兼容", () => {
    const a = tool("a", "agent-luban-uuid");
    const b = tool("b", "luban-uuid"); // 没 agent- 前缀但是同 uuid
    const out = groupConsecutiveToolCalls([a, b]);
    expect(out).toHaveLength(1);
    expect((out[0] as ToolGroupView).items).toHaveLength(2);
  });
});
