import { describe, expect, it } from "vitest";
import {
  applyTaskThreadEvent,
  type DeliverableMessage,
  type InlineFileMessage,
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
      ev({ type: "tool_call_started", tool: "Bash", args: "grep" }, { agent: LUBAN, id: "tc1", at: "2026-04-26T12:00:00Z" }),
      CTX,
    );
    s = applyTaskThreadEvent(
      s,
      ev(
        { type: "tool_call_finished", tool: "Bash", ok: true, output_preview: "result" },
        { agent: LUBAN, id: "tc2", at: "2026-04-26T12:00:00.5Z" },
      ),
      CTX,
    );
    expect(s).toHaveLength(1);
    const t = s[0] as ToolCallMessage;
    expect(t.agent).toBe(LUBAN);
    expect(t.status).toBe("ok");
    expect(t.duration_ms).toBe(500);
    // bug #76：reducer 之前用 input/output 字段读，但 wire 是 args/output_preview，
    // 字段名漂移导致 args_summary + output 永远 null → ToolCallCard 点不开。
    // 锁住映射防止回归。
    expect(t.args_summary).toBe("grep");
    expect(t.output).toBe("result");
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

  it("bug #76 system_origin · UserInterventionSent 带 origin → SystemMessage 玄女侧而非 user bubble", () => {
    const out = applyTaskThreadEvent(
      [],
      ev(
        {
          type: "user_intervention_sent",
          target: "worker-x",
          mode: "interrupt",
          text: "[REVIEW_REQUEST] 鲁班 待审...",
          mentions: [],
          system_origin: "review_request",
        } as ServerEvent["kind"],
        { agent: XUANNV, id: "sys-1" },
      ),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as { kind: string; origin?: string; text: string };
    expect(m.kind).toBe("system");
    expect(m.origin).toBe("review_request");
    expect(m.text).toContain("REVIEW_REQUEST");
  });

  it("bug #76 marker dedup · 紧邻同 text marker 折叠（防止 cc 多 turn agent_idle 灰横线泛滥）", () => {
    let s: Message[] = [];
    // 模拟 cc 多 turn 连发 agent_idle
    for (let i = 0; i < 5; i += 1) {
      s = applyTaskThreadEvent(
        s,
        ev({ type: "agent_idle" }, { agent: LUBAN, id: `mk-${i}` }),
        CTX,
      );
    }
    // 5 条全同 text → 只留首条
    const markers = s.filter((m) => m.kind === "marker");
    expect(markers).toHaveLength(1);
    // 中间夹一条不同的（worker bubble）→ 后续同 text marker 应该再次起新条
    s = applyTaskThreadEvent(
      s,
      ev({ type: "agent_responded", text: "回复" }, { agent: LUBAN, id: "ar-1" }),
      CTX,
    );
    s = applyTaskThreadEvent(
      s,
      ev({ type: "agent_idle" }, { agent: LUBAN, id: "mk-after" }),
      CTX,
    );
    const markers2 = s.filter((m) => m.kind === "marker");
    expect(markers2).toHaveLength(2);
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
      ev({ type: "tool_call_started", tool: "Bash", args: "grep" }, { agent: LUBAN, at: "2026-04-26T11:00:06Z", id: "h-5" }),
      ev(
        { type: "tool_call_finished", tool: "Bash", ok: true, output_preview: "..." },
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

  it("P2.7 · agent_inline_message_pushed · 起 InlineFileMessage 含 mime/body/filename", () => {
    const out = applyTaskThreadEvent(
      [],
      ev(
        {
          type: "agent_inline_message_pushed",
          task: "task-1",
          from: LUBAN,
          filename: "report.md",
          mime: "text/markdown",
          body: "# 报告\n\n查到 12 条",
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-inline-1" },
      ),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as InlineFileMessage;
    expect(m.kind).toBe("inline_file");
    expect(m.id).toBe("ev-inline-1");
    expect(m.agent).toBe(LUBAN);
    expect(m.role).toBe("luban");
    expect(m.role_display).toBe("鲁班");
    expect(m.filename).toBe("report.md");
    expect(m.mime).toBe("text/markdown");
    expect(m.body).toBe("# 报告\n\n查到 12 条");
  });

  it("P3 段 A · deliverable_produced · 起 DeliverableMessage 含 project/task/files", () => {
    const out = applyTaskThreadEvent(
      [],
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "erp",
          deliverable_kind: "research_summary",
          files: [
            { name: "report.md", sha256: "aaa", size_bytes: 1234 },
            { name: "data.csv", sha256: "bbb", size_bytes: 45678 },
          ],
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dv-1" },
      ),
      CTX,
    );
    expect(out).toHaveLength(1);
    const m = out[0] as DeliverableMessage;
    expect(m.kind).toBe("deliverable");
    expect(m.id).toBe("ev-dv-1");
    expect(m.agent).toBe(LUBAN);
    expect(m.role).toBe("luban");
    expect(m.role_display).toBe("鲁班");
    expect(m.deliverable_kind).toBe("research_summary");
    expect(m.project).toBe("erp");
    expect(m.task).toBe("task-1");
    expect(m.files).toHaveLength(2);
    expect(m.files[0]?.name).toBe("report.md");
    expect(m.files[1]?.size_bytes).toBe(45678);
  });

  it("P3 段 A · deliverable 缺 files / project 时 noop（防御）", () => {
    const before: Message[] = [];
    const noFiles = applyTaskThreadEvent(
      before,
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "erp",
          deliverable_kind: "code_change",
          files: [],
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dv-empty" },
      ),
      CTX,
    );
    expect(noFiles).toBe(before);
    const noProject = applyTaskThreadEvent(
      before,
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "",
          deliverable_kind: "code_change",
          files: [{ name: "x.md", sha256: "z", size_bytes: 1 }],
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dv-noproj" },
      ),
      CTX,
    );
    expect(noProject).toBe(before);
  });

  it("P3 段 A · 同 id 不重复 push（防 ws + history 双注同一 deliverable）", () => {
    const fst = applyTaskThreadEvent(
      [],
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "erp",
          deliverable_kind: "research_summary",
          files: [{ name: "x.md", sha256: "z", size_bytes: 1 }],
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dv-dup" },
      ),
      CTX,
    );
    const snd = applyTaskThreadEvent(
      fst,
      ev(
        {
          type: "deliverable_produced",
          task: "task-1",
          project: "erp",
          deliverable_kind: "research_summary",
          files: [{ name: "x.md", sha256: "z", size_bytes: 1 }],
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dv-dup" },
      ),
      CTX,
    );
    expect(snd).toHaveLength(1);
    expect(snd).toBe(fst);
  });

  it("P2.7 · 同 id 不重复 push（防 ws + history 双注同一 inline_file）", () => {
    const fst = applyTaskThreadEvent(
      [],
      ev(
        {
          type: "agent_inline_message_pushed",
          task: "task-1",
          from: LUBAN,
          filename: "a.md",
          mime: "text/markdown",
          body: "x",
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dup" },
      ),
      CTX,
    );
    const snd = applyTaskThreadEvent(
      fst,
      ev(
        {
          type: "agent_inline_message_pushed",
          task: "task-1",
          from: LUBAN,
          filename: "a.md",
          mime: "text/markdown",
          body: "x",
        } as ServerEvent["kind"],
        { agent: LUBAN, id: "ev-dup" },
      ),
      CTX,
    );
    expect(snd).toHaveLength(1);
    expect(snd).toBe(fst);
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
