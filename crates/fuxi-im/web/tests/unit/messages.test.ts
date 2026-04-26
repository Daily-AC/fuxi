import { describe, expect, it } from "vitest";
import {
  applyEvent,
  makeUserMessage,
  markUserMessage,
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
});
