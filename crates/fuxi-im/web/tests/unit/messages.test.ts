import { describe, expect, it } from "vitest";
import {
  applyEvent,
  fromStoredMessage,
  makeUserMessage,
  markUserMessage,
  mergeMessages,
  type Message,
} from "~/messages";
import type { ServerEvent } from "~/types/events";

// 嵌套 wire format · 跟实际后端 γ 推的一致：{ meta, kind }。
function ev(
  kind: ServerEvent["kind"],
  meta: Partial<ServerEvent["meta"]> = {},
): ServerEvent {
  return {
    meta: {
      id: meta.id ?? `id-${Math.random().toString(36).slice(2, 8)}`,
      at: meta.at ?? "2026-04-26T12:00:00Z",
      session: meta.session ?? null,
      agent: meta.agent ?? "f0d0f576-1234-4567-89ab-cdef01234567",
      task: meta.task ?? null,
    },
    kind,
  };
}

describe("messages.applyEvent · ServerEvent 嵌套", () => {
  it("agent_responded 没有 streaming bubble · 起新非 streaming bubble", () => {
    const out = applyEvent([], ev({ type: "agent_responded", text: "你好。需要帮忙？" }));
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({
      kind: "xuannv",
      text: "你好。需要帮忙？",
      streaming: false,
    });
  });

  it("thinking_started → 起空 streaming bubble（pulse 立即出现）", () => {
    const out = applyEvent([], ev({ type: "thinking_started" }));
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ kind: "xuannv", text: "", streaming: true });
  });

  it("thinking_started 然后 agent_responded · 完结同一 bubble，覆盖 text", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "thinking_started" }));
    s = applyEvent(s, ev({ type: "agent_responded", text: "好的，我看一下" }));
    expect(s).toHaveLength(1);
    expect((s[0] as { text: string }).text).toBe("好的，我看一下");
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
  });

  // Bug（2026-06-04 用户实测「消息重复显示两次，切 tab 复原」）：live 流渲染的
  // 玄女气泡 id 必须跟 conv_store 历史落库的 id 对齐——否则 loadHistory（onVisible
  // 不卸载时触发）merge 进同一条消息的历史副本时，mergeMessages 按 id 去重失败 →
  // 冒两条；切 tab 整页重挂载只剩历史 → 收敛回一条。
  // conv_store 给 message 发的是随机 uuid，但 source_event_id = AgentResponded 的
  // event id（handle_event:468）。所以两边都以 event id 作 id 才能对齐去重。
  it("Bug · live 玄女气泡 id = agent_responded event id（与历史 source_event_id 对齐）", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "thinking_started" }, { id: "evt-think" }));
    s = applyEvent(
      s,
      ev({ type: "agent_responded", text: "好的，已派给鲁班" }, { id: "evt-resp" }),
    );
    expect(s).toHaveLength(1);
    // 关键：完结 streaming bubble 必须采纳 agent_responded 的 event id，不能留
    // thinking_started 的 id（否则跟历史对不上）。
    expect(s[0]!.id).toBe("evt-resp");
  });

  it("Bug · fromStoredMessage 用 source_event_id 当 id（与 live 对齐）", () => {
    const stored = fromStoredMessage({
      id: "store-random-uuid",
      conv_id: "c",
      role: "xuannv",
      agent_id: null,
      kind: "text",
      content: { text: "好的，已派给鲁班" },
      attachments: undefined,
      source_event_id: "evt-resp",
      ts: "2026-06-04T12:00:00Z",
      topic_id: "t",
    } as unknown as Parameters<typeof fromStoredMessage>[0]);
    expect(stored).not.toBeNull();
    expect(stored!.id).toBe("evt-resp");
  });

  it("Bug · live 气泡 + 历史副本 mergeMessages 去重不重复（端到端）", () => {
    let live: Message[] = [];
    live = applyEvent(live, ev({ type: "thinking_started" }, { id: "evt-think" }));
    live = applyEvent(
      live,
      ev({ type: "agent_responded", text: "好的，已派给鲁班" }, { id: "evt-resp" }),
    );
    const stored = fromStoredMessage({
      id: "store-random-uuid",
      conv_id: "c",
      role: "xuannv",
      agent_id: null,
      kind: "text",
      content: { text: "好的，已派给鲁班" },
      attachments: undefined,
      source_event_id: "evt-resp",
      ts: "2026-06-04T12:00:00Z",
      topic_id: "t",
    } as unknown as Parameters<typeof fromStoredMessage>[0]);
    const merged = mergeMessages(live, [stored!]);
    expect(merged).toHaveLength(1);
  });

  it("thinking_started 然后 thinking_finished 没回复 · 空 bubble 被丢弃", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "thinking_started" }));
    s = applyEvent(s, ev({ type: "thinking_finished" }));
    expect(s).toHaveLength(0);
  });

  it("agent_text_delta 多次累积 · 同 streaming bubble", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "agent_text_delta", delta: "好" }));
    s = applyEvent(s, ev({ type: "agent_text_delta", delta: "的" }));
    s = applyEvent(s, ev({ type: "agent_text_delta", delta: "，我看一下" }));
    expect(s).toHaveLength(1);
    expect((s[0] as { text: string }).text).toBe("好的，我看一下");
    expect((s[0] as { streaming: boolean }).streaming).toBe(true);
  });

  it("agent_text_delta 累积 · 然后 agent_responded 整段覆盖 + 完结", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "agent_text_delta", delta: "好" }));
    s = applyEvent(s, ev({ type: "agent_responded", text: "好的，完整版本" }));
    expect(s).toHaveLength(1);
    expect((s[0] as { text: string }).text).toBe("好的，完整版本");
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
  });

  it("agent_idle / task_completed · 完结当前 streaming bubble", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "agent_text_delta", delta: "稍等" }));
    s = applyEvent(s, ev({ type: "agent_idle" }));
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
    expect((s[0] as { text: string }).text).toBe("稍等");
  });

  it("user_intervention_sent echo · optimistic 已渲染，忽略", () => {
    const u = makeUserMessage("hi");
    const before: Message[] = [u];
    const out = applyEvent(
      before,
      ev({ type: "user_intervention_sent", text: "hi" }),
    );
    expect(out).toBe(before);
  });

  it("makeUserMessage · pending=true，唯一 id", () => {
    const a = makeUserMessage("hi");
    const b = makeUserMessage("hi");
    expect(a.id).not.toBe(b.id);
    expect(a.pending).toBe(true);
    expect(a.text).toBe("hi");
  });

  it("markUserMessage · 按 id 改 pending/error", () => {
    const u = makeUserMessage("派活");
    const out = markUserMessage([u], u.id, { pending: false, error: "503" });
    expect(out[0]).toMatchObject({ pending: false, error: "503" });
  });

  it("turn 完结后再 thinking_started · 起新 bubble", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "agent_responded", text: "第一轮" }));
    s = applyEvent(s, ev({ type: "thinking_started" }));
    s = applyEvent(s, ev({ type: "agent_responded", text: "第二轮" }));
    expect(s).toHaveLength(2);
    expect((s[0] as { text: string }).text).toBe("第一轮");
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
    expect((s[1] as { text: string }).text).toBe("第二轮");
    expect((s[1] as { streaming: boolean }).streaming).toBe(false);
  });

  it("未知 kind type · noop（forward-compat 兜底）", () => {
    const before: Message[] = [];
    const out = applyEvent(before, ev({ type: "未来_新事件_变体" } as ServerEvent["kind"]));
    expect(out).toBe(before);
  });

  it("缺 kind / 缺 type · 防御性 noop 不崩", () => {
    const before: Message[] = [];
    expect(applyEvent(before, {} as unknown as ServerEvent)).toBe(before);
    expect(applyEvent(before, { meta: {} } as unknown as ServerEvent)).toBe(before);
  });

  it("Bug #24 · thinking_started → agent_responded \"\" · 丢空 bubble 不残留", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "thinking_started" }));
    expect(s).toHaveLength(1);
    s = applyEvent(s, ev({ type: "agent_responded", text: "" }));
    expect(s).toHaveLength(0);
  });

  it("Bug #24 · thinking_started → agent_responded 空白字符 · 丢", () => {
    let s: Message[] = [];
    s = applyEvent(s, ev({ type: "thinking_started" }));
    s = applyEvent(s, ev({ type: "agent_responded", text: "   \n  " }));
    expect(s).toHaveLength(0);
  });

  it("Bug #24 · agent_responded 空 text 没 streaming bubble · noop", () => {
    const before: Message[] = [];
    const out = applyEvent(before, ev({ type: "agent_responded", text: "" }));
    expect(out).toBe(before);
  });

  it("Bug #24 · 多轮空 turn 不堆 bubble", () => {
    let s: Message[] = [];
    for (let i = 0; i < 5; i += 1) {
      s = applyEvent(s, ev({ type: "thinking_started" }));
      s = applyEvent(s, ev({ type: "agent_responded", text: "" }));
    }
    expect(s).toHaveLength(0);
  });

  it("真实 fixture · 实测后端格式整段不崩", () => {
    const real: ServerEvent = {
      meta: {
        id: "xn-uuid-1",
        at: "2026-04-26T12:30:00Z",
        session: null,
        agent: "f0d0f576-fa97-4a0c-9c25-0123456789ab",
        task: "task-uuid-1",
      },
      kind: { type: "agent_responded", text: "你好。什么需要帮忙？" },
    };
    const out = applyEvent([], real);
    expect(out).toHaveLength(1);
    expect((out[0] as { text: string }).text).toBe("你好。什么需要帮忙？");
  });

  // bug #76：玄女主对话页之前不显示工具卡 + 思考。reducer 补 tool_call_started/finished
  // 处理后 + Conversation 加 tool_call 分支，玄女自己跑 Bash fuxi:* / Read 也能 inline
  // 看到。锁住字段映射（args + output_preview wire 名）防 #75 同款回归。
  it("bug #76 · tool_call_started + tool_call_finished 配对成 ToolCallMessage", () => {
    let s: Message[] = [];
    s = applyEvent(
      s,
      ev(
        { type: "tool_call_started", tool: "Bash", args: "fuxi spawn --role luban" },
        { id: "tc-s", at: "2026-05-04T01:00:00Z" },
      ),
    );
    s = applyEvent(
      s,
      ev(
        { type: "tool_call_finished", tool: "Bash", ok: true, output_preview: "agent-xxx" },
        { id: "tc-f", at: "2026-05-04T01:00:00.4Z" },
      ),
    );
    expect(s).toHaveLength(1);
    const t = s[0] as { kind: string; tool: string; status: string; args_summary: string; output: string };
    expect(t.kind).toBe("tool_call");
    expect(t.tool).toBe("Bash");
    expect(t.status).toBe("ok");
    expect(t.args_summary).toBe("fuxi spawn --role luban");
    expect(t.output).toBe("agent-xxx");
  });
});
