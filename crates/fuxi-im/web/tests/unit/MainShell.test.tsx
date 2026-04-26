import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { App } from "~/App";
import { createMockApi } from "../mocks/api";
import type { EventKind } from "~/types/events";

// 集成测：直接渲染 <App initialAuth="in"> 不可行（App 不接 prop），
// 转而构造 ApiProvider initialAuth=in 包 MainShell 子树。但 MainShell 不是 export
// —— 我们 render <App />，依赖 mock 的 fetchTasks 200 把 authState 推到 in。

afterEach(() => {
  setApiOverride(null);
});

function setup(opts?: { interveneSeq?: number[] }) {
  const api = createMockApi({ interveneSeq: opts?.interveneSeq });
  setApiOverride(api);
  const tools = render(() => <App />);
  return { api, ...tools };
}

describe("MainShell · intervene + WS 集成", () => {
  it("登入后输入发送 → optimistic user bubble 出现 + intervene 被调用", async () => {
    const { api, getByTestId, queryAllByTestId, unmount } = setup({ interveneSeq: [200] });
    // 等 ApiProvider probe 完成把 authState 推到 in
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByTestId("composer-input"), { target: { value: "派活：修 ERP" } });
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    expect(api.state.intervenes[0]?.text).toBe("派活：修 ERP");
    const userBubbles = queryAllByTestId("msg-user");
    expect(userBubbles.length).toBe(1);
    expect(userBubbles[0]?.textContent).toContain("派活：修 ERP");
    unmount();
  });

  it("503 一次 → 退避 1.5s → 重试成功 → user bubble 无 error", async () => {
    vi.useFakeTimers();
    const { api, getByTestId, queryByTestId, unmount } = setup({ interveneSeq: [503, 200] });
    // probe + render 用 real timer 等
    vi.useRealTimers();
    await new Promise((r) => setTimeout(r, 30));
    vi.useFakeTimers();
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("composer-send"));
    // 第一次 intervene 立刻发生（async microtask）
    await vi.advanceTimersByTimeAsync(0);
    expect(api.state.intervenes).toHaveLength(1);
    // 等退避 1500ms
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

  it("WS agent_text_delta 连续到达 → 玄女 bubble 累积渲染 + streaming pulse", async () => {
    const { api, queryByTestId, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    const evs: EventKind[] = [
      { type: "agent_text_delta", agent: "xuannv", delta: "好" },
      { type: "agent_text_delta", agent: "xuannv", delta: "的" },
      { type: "agent_text_delta", agent: "xuannv", delta: "，我看一下" },
    ];
    for (const e of evs) api.pushConv(e);
    await new Promise((r) => setTimeout(r, 30));
    const xnBubbles = queryAllByTestId("msg-xuannv");
    expect(xnBubbles).toHaveLength(1);
    expect(xnBubbles[0]?.textContent).toContain("好的，我看一下");
    expect(queryByTestId("msg-streaming")).toBeTruthy();
    // EndOfTurn 用 agent_idle 模拟
    api.pushConv({ type: "agent_idle", agent: "xuannv" });
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("msg-streaming")).toBeNull();
    unmount();
  });

  it("非玄女 agent_text_delta 阶段 2 不渲染（留阶段 3 门客）", async () => {
    const { api, queryByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushConv({ type: "agent_text_delta", agent: "luban", delta: "执行中" });
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("msg-xuannv")).toBeNull();
    unmount();
  });
});

// 防止 unused: ApiProvider import 是测试文档
void ApiProvider;
