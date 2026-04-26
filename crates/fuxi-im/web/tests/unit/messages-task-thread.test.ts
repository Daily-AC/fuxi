import { describe, expect, it } from "vitest";
import {
  applyTaskThreadEvent,
  type Message,
  type TaskThreadCtx,
  type ToolCallMessage,
  type UserMessage,
  type WorkerMessage,
  type XuannvMessage,
} from "~/messages";
import type { ServerEvent } from "~/types/events";

const XUANNV = "agent-xuannv";
const LUBAN = "agent-luban";
const PUSONG = "agent-pusong";

const CTX: TaskThreadCtx = {
  members: {
    [XUANNV]: { role: "xuannv", role_display: "玄女" },
    [LUBAN]: { role: "luban", role_display: "鲁班" },
    [PUSONG]: { role: "pusong", role_display: "蒲松" },
  },
  xuannv_id: XUANNV,
};

function ev(
  kind: ServerEvent["kind"],
  meta: Partial<ServerEvent["meta"]> = {},
): ServerEvent {
  return {
    meta: {
      id: meta.id ?? `id-${Math.random().toString(36).slice(2, 8)}`,
      at: meta.at ?? "2026-04-26T12:00:00Z",
      session: meta.session ?? null,
      agent: meta.agent === undefined ? null : meta.agent,
      task: meta.task ?? "task-1",
    },
    kind,
  };
}

describe("applyTaskThreadEvent · 任务 thread reducer (#39 / #N4')", () => {
  it("玄女 agent_responded · 起 XuannvMessage", () => {
    const out = applyTaskThreadEvent(
      [],
      ev({ type: "agent_responded", text: "好的" }, { agent: XUANNV }),
      CTX,
    );
    expect(out).toHaveLength(1);
    expect(out[0]?.kind).toBe("xuannv");
    const m = out[0] as XuannvMessage;
    expect(m.text).toBe("好的");
  });

  it("worker agent_responded · 起 WorkerMessage 带 role/role_display", () => {
    const out = applyTaskThreadEvent(
      [],
      ev({ type: "agent_responded", text: "查到 12 条" }, { agent: LUBAN }),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as WorkerMessage;
    expect(m.kind).toBe("worker");
    expect(m.role).toBe("luban");
    expect(m.role_display).toBe("鲁班");
    expect(m.text).toBe("查到 12 条");
  });

  it("agent 不在 members · 仍渲染但 role_display='门客' 兜底", () => {
    const out = applyTaskThreadEvent(
      [],
      ev({ type: "agent_responded", text: "未知" }, { agent: "agent-mystery" }),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as WorkerMessage;
    expect(m.role_display).toBe("门客");
  });

  it("UserInterventionSent · 右侧 user bubble + 还原 mentions", () => {
    const out = applyTaskThreadEvent(
      [],
      ev(
        {
          type: "user_intervention_sent",
          target: LUBAN,
          text: "查 ERP",
          mentions: [LUBAN, PUSONG],
        } as ServerEvent["kind"],
        { agent: XUANNV },
      ),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as UserMessage;
    expect(m.kind).toBe("user");
    expect(m.text).toBe("查 ERP");
    expect(m.mentions).toEqual([LUBAN, PUSONG]);
  });

  it("玄女 + worker delta 同时进 thread · 各自一条 streaming bubble", () => {
    let s: Message[] = [];
    s = applyTaskThreadEvent(
      s,
      ev({ type: "agent_text_delta", delta: "好" }, { agent: XUANNV, id: "t1" }),
      CTX,
    );
    s = applyTaskThreadEvent(
      s,
      ev({ type: "agent_text_delta", delta: "查" }, { agent: LUBAN, id: "t2" }),
      CTX,
    );
    expect(s).toHaveLength(2);
    expect((s[0] as XuannvMessage).agent).toBe(XUANNV);
    expect((s[0] as XuannvMessage).text).toBe("好");
    expect((s[1] as WorkerMessage).agent).toBe(LUBAN);
    expect((s[1] as WorkerMessage).text).toBe("查");
  });

  it("worker tool_call + tool_result · ToolCallCard agent 标记正确", () => {
    let s: Message[] = [];
    s = applyTaskThreadEvent(
      s,
      ev({ type: "tool_call", tool: "Bash", input: "grep" }, { agent: LUBAN, id: "tc1", at: "2026-04-26T12:00:00Z" }),
      CTX,
    );
    s = applyTaskThreadEvent(
      s,
      ev(
        { type: "tool_result", tool: "Bash", ok: true, output: "result" },
        { agent: LUBAN, id: "tc2", at: "2026-04-26T12:00:00.5Z" },
      ),
      CTX,
    );
    expect(s).toHaveLength(1);
    const t = s[0] as ToolCallMessage;
    expect(t.agent).toBe(LUBAN);
    expect(t.status).toBe("ok");
    expect(t.duration_ms).toBe(500);
  });

  it("agent_idle · marker 文案带 role_display", () => {
    const out = applyTaskThreadEvent([], ev({ type: "agent_idle" }, { agent: LUBAN }), CTX);
    expect(out).toHaveLength(1);
    expect(out[0]?.kind).toBe("marker");
    expect((out[0] as { text: string }).text).toContain("鲁班");
    expect((out[0] as { text: string }).text).toContain("idle");
  });

  it("task_completed summary · marker 文案带 summary", () => {
    const out = applyTaskThreadEvent(
      [],
      ev({ type: "task_completed", summary: "找到 12 条" }, { agent: XUANNV }),
      CTX,
    );
    expect(out).toHaveLength(1);
    expect((out[0] as { text: string }).text).toContain("找到 12 条");
  });

  it("task_state_changed Done · marker 任务完成", () => {
    const out = applyTaskThreadEvent(
      [],
      ev({ type: "task_state_changed", to: "Done" } as ServerEvent["kind"], { agent: XUANNV }),
      CTX,
    );
    expect(out).toHaveLength(1);
    expect((out[0] as { text: string }).text).toContain("任务完成");
  });

  it("history fold · 多 agent 多事件混合 reduce 出有序时间线", () => {
    const events: ServerEvent[] = [
      ev(
        {
          type: "user_intervention_sent",
          target: XUANNV,
          text: "查 ERP API",
          mentions: [],
        } as ServerEvent["kind"],
        { agent: XUANNV, at: "2026-04-26T11:00:00Z", id: "h-1" },
      ),
      ev({ type: "agent_responded", text: "好的，派给鲁班" }, { agent: XUANNV, at: "2026-04-26T11:00:01Z", id: "h-2" }),
      ev({ type: "thinking_started" }, { agent: LUBAN, at: "2026-04-26T11:00:02Z", id: "h-3" }),
      ev({ type: "thinking_finished" }, { agent: LUBAN, at: "2026-04-26T11:00:05Z", id: "h-4" }),
      ev({ type: "tool_call", tool: "Bash", input: "grep" }, { agent: LUBAN, at: "2026-04-26T11:00:06Z", id: "h-5" }),
      ev(
        { type: "tool_result", tool: "Bash", ok: true, output: "..." },
        { agent: LUBAN, at: "2026-04-26T11:00:06.5Z", id: "h-6" },
      ),
      ev({ type: "agent_responded", text: "查到 12 条" }, { agent: LUBAN, at: "2026-04-26T11:00:07Z", id: "h-7" }),
      ev({ type: "task_completed", summary: "完工" }, { agent: XUANNV, at: "2026-04-26T11:00:08Z", id: "h-8" }),
    ];
    const folded = events.reduce<Message[]>((acc, e) => applyTaskThreadEvent(acc, e, CTX), []);
    expect(folded.map((m) => m.kind)).toEqual([
      "user",
      "xuannv",
      "thinking",
      "tool_call",
      "worker",
      "marker",
    ]);
  });

  it("无 agent 的事件 · noop（防御）", () => {
    const before: Message[] = [];
    const out = applyTaskThreadEvent(
      before,
      ev({ type: "agent_responded", text: "野生" }, { agent: null }),
      CTX,
    );
    expect(out).toBe(before);
  });

  it("空 text 玄女 turn · 丢空 bubble（Bug #24 同款）", () => {
    let s: Message[] = [];
    s = applyTaskThreadEvent(s, ev({ type: "thinking_started" }, { agent: XUANNV }), CTX);
    expect(s).toHaveLength(1);
    s = applyTaskThreadEvent(s, ev({ type: "agent_responded", text: "" }, { agent: XUANNV }), CTX);
    // thinking 还在（reducer 这里只看 streaming bubble，thinking 单独走）
    // 但应该没有空 xuannv bubble
    const xn = s.filter((m) => m.kind === "xuannv");
    expect(xn).toHaveLength(0);
  });
});
