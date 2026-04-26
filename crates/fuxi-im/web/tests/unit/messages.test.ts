import { describe, expect, it } from "vitest";
import {
  applyEvent,
  makeUserMessage,
  markUserMessage,
  type Message,
} from "~/messages";

describe("messages.applyEvent", () => {
  it("agent_text_delta · 起新玄女 bubble，streaming=true", () => {
    const out = applyEvent([], {
      type: "agent_text_delta",
      agent: "xuannv",
      delta: "好",
      ts: "2026-04-26T12:00:00Z",
    });
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ kind: "xuannv", text: "好", streaming: true });
  });

  it("agent_text_delta 多次 · 累积同一 bubble", () => {
    let s: Message[] = [];
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "好" });
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "的" });
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "，" });
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "我看一下" });
    expect(s).toHaveLength(1);
    expect((s[0] as { text: string }).text).toBe("好的，我看一下");
    expect((s[0] as { streaming: boolean }).streaming).toBe(true);
  });

  it("agent_responded · 完结当前 bubble，覆盖 text", () => {
    let s: Message[] = [];
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "好的" });
    s = applyEvent(s, {
      type: "agent_responded",
      agent: "xuannv",
      text: "好的，我看一下 ERP 任务",
    });
    expect(s).toHaveLength(1);
    expect((s[0] as { text: string }).text).toBe("好的，我看一下 ERP 任务");
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
  });

  it("agent_idle / result_success · 完结 streaming bubble", () => {
    let s: Message[] = [];
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "稍等" });
    s = applyEvent(s, { type: "agent_idle", agent: "xuannv" });
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
    expect((s[0] as { text: string }).text).toBe("稍等");
  });

  it("非玄女 agent_text_delta · 阶段 2 忽略（留阶段 3）", () => {
    const out = applyEvent([], {
      type: "agent_text_delta",
      agent: "luban",
      delta: "执行中",
    });
    expect(out).toEqual([]);
  });

  it("user_message echo · 服务端 echo 不重复渲染", () => {
    const u = makeUserMessage("hi");
    const before: Message[] = [u];
    const out = applyEvent(before, { type: "user_message", text: "hi" });
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
    const before: Message[] = [u, u];
    void before;
    const arr: Message[] = [u];
    const out = markUserMessage(arr, u.id, { pending: false, error: "503" });
    expect(out[0]).toMatchObject({ pending: false, error: "503" });
  });

  it("agent_text_delta 后切流（streaming bubble 已完结）→ 起新 bubble", () => {
    let s: Message[] = [];
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "好" });
    s = applyEvent(s, { type: "agent_idle", agent: "xuannv" }); // 完结
    s = applyEvent(s, { type: "agent_text_delta", agent: "xuannv", delta: "再说一下" });
    expect(s).toHaveLength(2);
    expect((s[0] as { streaming: boolean }).streaming).toBe(false);
    expect((s[1] as { streaming: boolean }).streaming).toBe(true);
    expect((s[1] as { text: string }).text).toBe("再说一下");
  });
});
