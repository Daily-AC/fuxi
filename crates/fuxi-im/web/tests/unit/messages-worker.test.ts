import { describe, expect, it } from "vitest";
import {
  applyWorkerEvent,
  type DeliverableMessage,
  type Message,
  type ToolCallMessage,
  type ThinkingMessage,
  type WorkerMessage,
  type WorkerReducerCtx,
} from "~/messages";
import type { ServerEvent } from "~/types/events";

const LUBAN = "agent-luban-uuid";
const MO = "agent-mozi-uuid";

const CTX: WorkerReducerCtx = { agent: LUBAN, role_display: "鲁班" };

function ev(
  kind: ServerEvent["kind"],
  meta: Partial<ServerEvent["meta"]> = {},
): ServerEvent {
  return {
    meta: {
      id: meta.id ?? `id-${Math.random().toString(36).slice(2, 8)}`,
      at: meta.at ?? "2026-04-26T12:00:00Z",
      session: meta.session ?? null,
      agent: meta.agent === undefined ? LUBAN : meta.agent,
      task: meta.task ?? null,
    },
    kind,
  };
}

describe("applyWorkerEvent · 私聊页 reducer (#N3)", () => {
  it("agent_responded · 起 WorkerMessage（不是 XuannvMessage）", () => {
    const out = applyWorkerEvent([], ev({ type: "agent_responded", text: "我看了 ERP API" }), CTX);
    expect(out).toHaveLength(1);
    const m = out[0] as WorkerMessage;
    expect(m.kind).toBe("worker");
    expect(m.text).toBe("我看了 ERP API");
    expect(m.role_display).toBe("鲁班");
    expect(m.agent).toBe(LUBAN);
    expect(m.streaming).toBe(false);
  });

  it("非该 agent 的事件丢（filter 防御）", () => {
    const out = applyWorkerEvent(
      [],
      ev({ type: "agent_responded", text: "墨子说话" }, { agent: MO }),
      CTX,
    );
    expect(out).toHaveLength(0);
  });

  it("agent_text_delta 累积 · 同 streaming bubble", () => {
    let s: Message[] = [];
    s = applyWorkerEvent(s, ev({ type: "agent_text_delta", delta: "好" }), CTX);
    s = applyWorkerEvent(s, ev({ type: "agent_text_delta", delta: "的" }), CTX);
    expect(s).toHaveLength(1);
    const m = s[0] as WorkerMessage;
    expect(m.text).toBe("好的");
    expect(m.streaming).toBe(true);
  });

  it("thinking_started + thinking_finished · ThinkingMessage 折叠条 + duration", () => {
    let s: Message[] = [];
    s = applyWorkerEvent(
      s,
      ev({ type: "thinking_started" }, { at: "2026-04-26T12:00:00Z", id: "th-1" }),
      CTX,
    );
    expect(s).toHaveLength(1);
    expect(s[0]?.kind).toBe("thinking");
    expect((s[0] as ThinkingMessage).streaming).toBe(true);
    s = applyWorkerEvent(
      s,
      ev({ type: "thinking_finished" }, { at: "2026-04-26T12:00:05Z", id: "th-2" }),
      CTX,
    );
    expect(s).toHaveLength(1);
    const t = s[0] as ThinkingMessage;
    expect(t.streaming).toBe(false);
    expect(t.duration_ms).toBe(5000);
  });

  it("tool_call + tool_result · 配对成单 ToolCallMessage 完结态", () => {
    let s: Message[] = [];
    s = applyWorkerEvent(
      s,
      ev(
        { type: "tool_call_started", tool: "Bash", args: "grep server/api/v1.go" },
        { at: "2026-04-26T12:00:00Z", id: "tc-1" },
      ),
      CTX,
    );
    expect(s).toHaveLength(1);
    expect((s[0] as ToolCallMessage).status).toBe("running");
    s = applyWorkerEvent(
      s,
      ev(
        { type: "tool_call_finished", tool: "Bash", ok: true, output_preview: "main.go" },
        { at: "2026-04-26T12:00:00.400Z", id: "tc-2" },
      ),
      CTX,
    );
    // 仍是单条（配对覆盖）
    expect(s).toHaveLength(1);
    const t = s[0] as ToolCallMessage;
    expect(t.status).toBe("ok");
    expect(t.duration_ms).toBe(400);
    expect(t.output).toBe("main.go");
  });

  it("tool_result 没有对应 started · 单独插一条完结卡", () => {
    const s = applyWorkerEvent(
      [],
      ev({ type: "tool_call_finished", tool: "Bash", ok: false }, { id: "tc-orphan" }),
      CTX,
    );
    expect(s).toHaveLength(1);
    expect((s[0] as ToolCallMessage).status).toBe("err");
  });

  it("UserInterventionSent target=该 agent · 起 user bubble", () => {
    const s = applyWorkerEvent(
      [],
      ev(
        { type: "user_intervention_sent", target: LUBAN, text: "查 ERP" } as ServerEvent["kind"],
        // 注意：spec §workers.rs · UserInterventionSent 走 target，meta.agent 可能是玄女
        { agent: "agent-xuannv-uuid", id: "u-1" },
      ),
      CTX,
    );
    expect(s).toHaveLength(1);
    expect(s[0]?.kind).toBe("user");
  });

  it("UserInterventionSent target≠该 agent · 丢", () => {
    const s = applyWorkerEvent(
      [],
      ev(
        { type: "user_intervention_sent", target: MO, text: "给墨子的" } as ServerEvent["kind"],
        { agent: "agent-xuannv-uuid" },
      ),
      CTX,
    );
    expect(s).toHaveLength(0);
  });

  it("agent_idle · 居中 marker", () => {
    const s = applyWorkerEvent([], ev({ type: "agent_idle" }), CTX);
    expect(s).toHaveLength(1);
    expect(s[0]?.kind).toBe("marker");
    expect((s[0] as { text: string }).text).toContain("idle");
  });

  it("task_completed summary · marker 文案带 summary", () => {
    const s = applyWorkerEvent(
      [],
      ev({ type: "task_completed", summary: "找到 API · 12 条" }),
      CTX,
    );
    expect(s).toHaveLength(1);
    expect((s[0] as { text: string }).text).toContain("找到 API");
  });

  it("thinking_started 没 finished 直接 agent_responded · ThinkingMessage 留着 streaming，bubble 起新", () => {
    let s: Message[] = [];
    s = applyWorkerEvent(s, ev({ type: "thinking_started" }), CTX);
    s = applyWorkerEvent(s, ev({ type: "agent_responded", text: "结果出来了" }), CTX);
    // thinking 留着（未 finished）+ worker bubble 一条
    expect(s).toHaveLength(2);
    expect(s[0]?.kind).toBe("thinking");
    expect(s[1]?.kind).toBe("worker");
  });

  it("未知 kind type · noop", () => {
    const before: Message[] = [];
    const out = applyWorkerEvent(
      before,
      ev({ type: "未来_新事件" } as ServerEvent["kind"]),
      CTX,
    );
    expect(out).toBe(before);
  });

  it("P3 段 A · deliverable_produced · 起 DeliverableMessage 含 files", () => {
    const out = applyWorkerEvent(
      [],
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "erp",
          deliverable_kind: "code_change",
          files: [{ name: "patch.diff", sha256: "x", size_bytes: 512 }],
        } as ServerEvent["kind"],
        { id: "ev-dv-w" },
      ),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as DeliverableMessage;
    expect(m.kind).toBe("deliverable");
    expect(m.id).toBe("ev-dv-w");
    expect(m.role_display).toBe("鲁班");
    expect(m.deliverable_kind).toBe("code_change");
    expect(m.project).toBe("erp");
    expect(m.task).toBe("task-1");
    expect(m.files).toHaveLength(1);
    expect(m.files[0]?.name).toBe("patch.diff");
  });

  it("P3 段 A · deliverable agent 不匹配 · 直接丢", () => {
    const before: Message[] = [];
    const out = applyWorkerEvent(
      before,
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "erp",
          deliverable_kind: "test_result",
          files: [{ name: "out.log", sha256: "y", size_bytes: 200 }],
        } as ServerEvent["kind"],
        { agent: MO, id: "ev-dv-other" },
      ),
      CTX,
    );
    expect(out).toBe(before);
  });

  it("history fold · 多事件 reduce 出有序时间线", () => {
    const events: ServerEvent[] = [
      ev(
        { type: "user_intervention_sent", target: LUBAN, text: "查 ERP" } as ServerEvent["kind"],
        { agent: "agent-xuannv-uuid", at: "2026-04-26T11:00:00Z", id: "h-1" },
      ),
      ev({ type: "thinking_started" }, { at: "2026-04-26T11:00:01Z", id: "h-2" }),
      ev({ type: "thinking_finished" }, { at: "2026-04-26T11:00:03Z", id: "h-3" }),
      ev(
        { type: "tool_call_started", tool: "Read", args: "main.go" },
        { at: "2026-04-26T11:00:04Z", id: "h-4" },
      ),
      ev(
        { type: "tool_call_finished", tool: "Read", ok: true, output_preview: "..." },
        { at: "2026-04-26T11:00:04.5Z", id: "h-5" },
      ),
      ev({ type: "agent_responded", text: "找到了" }, { at: "2026-04-26T11:00:05Z", id: "h-6" }),
      ev({ type: "agent_idle" }, { at: "2026-04-26T11:00:06Z", id: "h-7" }),
    ];
    const folded = events.reduce<Message[]>((acc, e) => applyWorkerEvent(acc, e, CTX), []);
    // user, thinking(完结), tool_call(完结), worker, marker = 5 条
    expect(folded.map((m) => m.kind)).toEqual([
      "user",
      "thinking",
      "tool_call",
      "worker",
      "marker",
    ]);
  });
});
