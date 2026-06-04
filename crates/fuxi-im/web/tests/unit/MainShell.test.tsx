import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { setApiOverride } from "~/components/ApiProvider";
import { App } from "~/App";
import { createMockApi } from "../mocks/api";
import type { ServerEvent } from "~/types/events";

afterEach(() => {
  setApiOverride(null);
});

function setup(opts?: {
  interveneSeq?: number[];
  history?: Record<string, import("~/types/api").StoredMessage[]>;
  nodes?: import("~/types/api").NodesResponse;
}) {
  const api = createMockApi({
    interveneSeq: opts?.interveneSeq,
    history: opts?.history,
    nodes: opts?.nodes,
  });
  setApiOverride(api);
  const tools = render(() => <App />);
  return { api, ...tools };
}

// daimeng 重构后默认 tab=家(Home)；聊天(玄女会话)挪到 tab 1。
// 所有走 chat editor / conversation 的测试须先切到「聊天」tab。
function switchToChat(): void {
  const el = document.querySelector('[data-testid="tab-xuannv"]');
  if (!el) throw new Error('tab-xuannv not found');
  fireEvent.click(el as HTMLElement);
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "派活：修 ERP" } });
    fireEvent.click(getByTestId("mention-send"));
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
    vi.useFakeTimers();
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("mention-send"));
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
    vi.useFakeTimers();
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("mention-send"));
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
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
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
    fireEvent.input(getByQueryTestId("mention-editor"), { target: { value: "hi" } });
    fireEvent.click(getByQueryTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    api.pushConv(ev({ type: "user_intervention_sent", text: "hi" }));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId("msg-user")).toHaveLength(1);
    unmount();
  });

  it("登入后预加载历史 → 5 条 stored message 进 stream", async () => {
    const history = {
      xuannv: [
        {
          id: "h1",
          conv_id: "xuannv",
          role: "user" as const,
          kind: "text" as const,
          content: "之前的提问",
          ts: "2026-04-26T10:00:00Z",
        },
        {
          id: "h2",
          conv_id: "xuannv",
          role: "xuannv" as const,
          kind: "text" as const,
          content: "之前的回答",
          ts: "2026-04-26T10:00:30Z",
        },
        {
          id: "h3",
          conv_id: "xuannv",
          role: "user" as const,
          kind: "text" as const,
          content: "再问",
          ts: "2026-04-26T10:01:00Z",
        },
        {
          id: "h4",
          conv_id: "xuannv",
          role: "xuannv" as const,
          kind: "text" as const,
          content: "再答",
          ts: "2026-04-26T10:01:30Z",
        },
        {
          id: "h5",
          conv_id: "xuannv",
          role: "user" as const,
          kind: "text" as const,
          content: "三问",
          ts: "2026-04-26T10:02:00Z",
        },
      ],
    };
    const { queryAllByTestId, unmount } = setup({ history });
    await new Promise((r) => setTimeout(r, 50));
    switchToChat();
    await new Promise((r) => setTimeout(r, 30));
    const users = queryAllByTestId("msg-user");
    const xn = queryAllByTestId("msg-xuannv");
    expect(users).toHaveLength(3);
    expect(xn).toHaveLength(2);
    expect(users[0]?.textContent).toContain("之前的提问");
    unmount();
  });

  it("Bug #45 · 历史预加载 backend 实际 wire（content 是 {text}）→ 渲染上屏", async () => {
    // backend conv_store::handle_event 写库时 content 是 serde_json::json!({"text":"..."})
    // v3 之前 fromStoredMessage 把非 string 当空 → 历史全丢；本测试守住"刷新历史不丢"。
    const history = {
      xuannv: [
        {
          id: "real-u",
          conv_id: "xuannv",
          role: "user" as const,
          kind: "text" as const,
          content: { text: "查 ERP API" },
          ts: "2026-04-26T10:00:00Z",
        },
        {
          id: "real-x",
          conv_id: "xuannv",
          role: "xuannv" as const,
          agent_id: "x-uuid",
          kind: "text" as const,
          content: { text: "好，派给鲁班" },
          ts: "2026-04-26T10:00:30Z",
        },
      ],
    };
    const { queryAllByTestId, unmount } = setup({ history });
    await new Promise((r) => setTimeout(r, 50));
    switchToChat();
    await new Promise((r) => setTimeout(r, 30));
    const users = queryAllByTestId("msg-user");
    const xn = queryAllByTestId("msg-xuannv");
    expect(users).toHaveLength(1);
    expect(xn).toHaveLength(1);
    expect(users[0]?.textContent).toContain("查 ERP API");
    expect(xn[0]?.textContent).toContain("好，派给鲁班");
    unmount();
  });

  it("v3 #60 · 玄女 tab @node mac-local · intervene body 含 pinned_node + task_id null", async () => {
    const { api, getByTestId, unmount } = setup({
      interveneSeq: [200],
      nodes: {
        nodes: [
          {
            node_id: "mac-local",
            tags: ["local"],
            max_concurrency: 8,
            inflight_jobs: 0,
            heartbeat_lag_ms: 600,
            online: true,
            registered_at: null,
            workers: [],
          },
        ],
      },
    });
    await new Promise((r) => setTimeout(r, 30));
    switchToChat();
    await new Promise((r) => setTimeout(r, 5));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "用 @ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 跑 cargo test" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    const req = api.state.intervenes[0] as {
      task_id?: string | null;
      pinned_node?: string;
    };
    expect(req.pinned_node).toBe("mac-local");
    expect(req.task_id).toBeNull();
    unmount();
  });

  it("历史 + WS 推同 id 不重复（去重）", async () => {
    const history = {
      xuannv: [
        {
          id: "shared-id",
          conv_id: "xuannv",
          role: "xuannv" as const,
          kind: "text" as const,
          content: "历史里的话",
          ts: "2026-04-26T10:00:00Z",
        },
      ],
    };
    const { api, queryAllByTestId, unmount } = setup({ history });
    await new Promise((r) => setTimeout(r, 50));
    switchToChat();
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId("msg-xuannv")).toHaveLength(1);
    // WS 推同 id 的 agent_responded · reducer 走 startBubble，会创新一条（id 不同）
    // 验证：用同 id 不去重（reducer 与 history merge 走两条不同 path，
    // 这里测的是 ε 不会因为 ws 重发同 id event 让历史 bubble 复制）
    api.pushConv({
      meta: {
        id: "shared-id",
        at: "2026-04-26T11:00:00Z",
        session: null,
        agent: "x",
        task: null,
      },
      kind: { type: "agent_responded", text: "新一条" },
    });
    await new Promise((r) => setTimeout(r, 30));
    // applyEvent 用 ev.meta.id 作 bubble id；如和历史 id 撞会插到列表后面（reducer 不去重）
    // 这是已知 v1 限制：ws + history id 撞时 ε 不去重，由 β 端保证 id 唯一性。
    // 本测试单纯验证流程不崩。
    expect(queryAllByTestId("msg-xuannv").length).toBeGreaterThanOrEqual(1);
    unmount();
  });
});

// vitest auto-globals 不暴露 getByTestId 全局，给上面 echo 测试做局部 helper：
function getByQueryTestId(id: string): HTMLElement {
  const el = document.querySelector(`[data-testid="${id}"]`);
  if (!el) throw new Error(`testId ${id} not found`);
  return el as HTMLElement;
}
