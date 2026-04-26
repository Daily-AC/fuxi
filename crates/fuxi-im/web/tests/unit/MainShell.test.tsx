import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { setApiOverride } from "~/components/ApiProvider";
import { App } from "~/App";
import { createMockApi } from "../mocks/api";
import type { ServerEvent } from "~/types/events";

afterEach(() => {
  setApiOverride(null);
});

function setup(opts?: { interveneSeq?: number[] }) {
  const api = createMockApi({ interveneSeq: opts?.interveneSeq });
  setApiOverride(api);
  const tools = render(() => <App />);
  return { api, ...tools };
}

function ev(kind: ServerEvent["kind"]): ServerEvent {
  return {
    meta: {
      id: `id-${Math.random().toString(36).slice(2, 8)}`,
      at: new Date().toISOString(),
      session: null,
      agent: "f0d0f576-fa97-4a0c-9c25-test",
      task: null,
    },
    kind,
  };
}

describe("MainShell · intervene + WS 集成（嵌套 wire format）", () => {
  it("登入后输入发送 → optimistic user bubble + intervene 被调用", async () => {
    const { api, getByTestId, queryAllByTestId, unmount } = setup({ interveneSeq: [200] });
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByTestId("composer-input"), { target: { value: "派活：修 ERP" } });
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    expect(api.state.intervenes[0]?.text).toBe("派活：修 ERP");
    const userBubbles = queryAllByTestId("msg-user");
    expect(userBubbles).toHaveLength(1);
    expect(userBubbles[0]?.textContent).toContain("派活：修 ERP");
    unmount();
  });

  it("503 一次 → 退避 1.5s → 重试成功 → user bubble 无 error", async () => {
    vi.useFakeTimers();
    const { api, getByTestId, queryByTestId, unmount } = setup({ interveneSeq: [503, 200] });
    vi.useRealTimers();
    await new Promise((r) => setTimeout(r, 30));
    vi.useFakeTimers();
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("composer-send"));
    await vi.advanceTimersByTimeAsync(0);
    expect(api.state.intervenes).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1600);
    expect(api.state.intervenes).toHaveLength(2);
    expect(queryByTestId("msg-user-error")).toBeNull();
    vi.useRealTimers();
    unmount();
  });

  it("503 两次 → bubble 下挂红字 inline 错误", async () => {
    vi.useFakeTimers();
    const { getByTestId, unmount } = setup({ interveneSeq: [503, 503] });
    vi.useRealTimers();
    await new Promise((r) => setTimeout(r, 30));
    vi.useFakeTimers();
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("composer-send"));
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1600);
    vi.useRealTimers();
    await new Promise((r) => setTimeout(r, 10));
    expect(getByTestId("msg-user-error").textContent).toMatch(/玄女后端不在|服务暂忙|503/);
    unmount();
  });

  it("WS agent_responded（cc haiku 实际格式）→ 玄女 bubble 整段渲染", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushConv(ev({ type: "agent_responded", text: "你好。什么需要帮忙？" }));
    await new Promise((r) => setTimeout(r, 30));
    const xn = queryAllByTestId("msg-xuannv");
    expect(xn).toHaveLength(1);
    expect(xn[0]?.textContent).toContain("你好。什么需要帮忙？");
    unmount();
  });

  it("WS thinking_started → pulse 立即出现；agent_responded → pulse 消失", async () => {
    const { api, queryByTestId, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushConv(ev({ type: "thinking_started" }));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("msg-streaming")).toBeTruthy();
    api.pushConv(ev({ type: "agent_responded", text: "好的" }));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("msg-streaming")).toBeNull();
    const xn = queryAllByTestId("msg-xuannv");
    expect(xn).toHaveLength(1);
    expect(xn[0]?.textContent).toContain("好的");
    unmount();
  });

  it("WS agent_text_delta 连续 → 累积同一 streaming bubble", async () => {
    const { api, queryByTestId, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushConv(ev({ type: "agent_text_delta", delta: "好" }));
    api.pushConv(ev({ type: "agent_text_delta", delta: "的" }));
    api.pushConv(ev({ type: "agent_text_delta", delta: "，我看一下" }));
    await new Promise((r) => setTimeout(r, 30));
    const xn = queryAllByTestId("msg-xuannv");
    expect(xn).toHaveLength(1);
    expect(xn[0]?.textContent).toContain("好的，我看一下");
    expect(queryByTestId("msg-streaming")).toBeTruthy();
    api.pushConv(ev({ type: "agent_idle" }));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("msg-streaming")).toBeNull();
    unmount();
  });

  it("user_intervention_sent echo · 不重复渲染 user bubble", async () => {
    const { api, queryAllByTestId, unmount } = setup({ interveneSeq: [200] });
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByQueryTestId("composer-input"), { target: { value: "hi" } });
    fireEvent.click(getByQueryTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 30));
    api.pushConv(ev({ type: "user_intervention_sent", text: "hi" }));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId("msg-user")).toHaveLength(1);
    unmount();
  });
});

// vitest auto-globals 不暴露 getByTestId 全局，给上面 echo 测试做局部 helper：
function getByQueryTestId(id: string): HTMLElement {
  const el = document.querySelector(`[data-testid="${id}"]`);
  if (!el) throw new Error(`testId ${id} not found`);
  return el as HTMLElement;
}
