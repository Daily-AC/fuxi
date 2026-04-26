import { test, expect } from "@playwright/test";

// v2 阶段 1+2：视觉骨架 + 玄女/user message + intervene + WS 流式。

declare global {
  interface Window {
    __FUXI_FETCH__: Array<{ input: string }>;
    __FUXI_WS__: { last: WebSocketLike | null };
  }
}

interface WebSocketLike {
  dispatchEvent(e: Event): boolean;
  url: string;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const fetchCalls: Array<{ input: string }> = [];
    window.__FUXI_FETCH__ = fetchCalls;
    window.__FUXI_WS__ = { last: null };

    const json = (data: unknown, status = 200): Response =>
      new Response(JSON.stringify(data), {
        status,
        headers: { "content-type": "application/json" },
      });

    const realFetch = window.fetch.bind(window);
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      fetchCalls.push({ input: url });
      if (url.startsWith("/api/tasks")) {
        if (url.includes("/events")) return json({ events: [], next_cursor: null });
        return json({ tasks: [] });
      }
      if (url === "/api/intervene") return json({ ok: true });
      if (url === "/api/push/vapid-pub") return json({ public_key: "stub" });
      if (url === "/api/push/subscribe") return json({ ok: true });
      return realFetch(input as RequestInfo, init);
    };

    class FakeWS extends EventTarget {
      static OPEN = 1;
      static CLOSED = 3;
      readyState = 0;
      url: string;
      constructor(url: string) {
        super();
        this.url = url;
        // 暴露最近一个 socket 给 e2e 推消息
        window.__FUXI_WS__.last = this;
        setTimeout(() => {
          this.readyState = 1;
          this.dispatchEvent(new Event("open"));
        }, 30);
      }
      send(): void {}
      close(): void {
        if (this.readyState === 3) return;
        this.readyState = 3;
        this.dispatchEvent(new Event("close"));
      }
    }
    (window as unknown as { WebSocket: typeof FakeWS }).WebSocket = FakeWS;
  });
});

test("登入后 main shell 出现，Header + Composer + 空态", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId("header-tasks")).toBeVisible();
  await expect(page.getByTestId("header-center")).toContainText("玄女");
  await expect(page.getByTestId("header-nodes")).toBeVisible();
  await expect(page.getByTestId("conversation-empty")).toContainText("玄女在线");
  await expect(page.getByTestId("composer-input")).toBeVisible();
});

test("composer 输入后发送按钮变 active", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  const send = page.getByTestId("composer-send");
  await expect(send).toBeDisabled();
  await page.getByTestId("composer-input").fill("hi");
  await expect(send).toBeEnabled();
});

test("发送消息 → user bubble + intervene 调用 + WS 流式收到玄女回复", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  // 等 ws open
  await page.waitForFunction(() => window.__FUXI_WS__.last !== null);

  // 输入并发送
  await page.getByTestId("composer-input").fill("hi 玄女");
  await page.getByTestId("composer-send").click();

  // optimistic：user bubble 立刻出现
  await expect(page.getByTestId("msg-user")).toContainText("hi 玄女");

  // intervene fetch 被调用
  await expect
    .poll(async () =>
      page.evaluate(() => window.__FUXI_FETCH__.filter((c) => c.input === "/api/intervene").length),
    )
    .toBeGreaterThan(0);

  // 服务端推 agent_text_delta 三段
  await page.evaluate(() => {
    const ws = window.__FUXI_WS__.last as unknown as EventTarget;
    const send = (data: unknown): void => {
      ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(data) }));
    };
    send({ type: "agent_text_delta", agent: "xuannv", delta: "好" });
    send({ type: "agent_text_delta", agent: "xuannv", delta: "的" });
    send({ type: "agent_text_delta", agent: "xuannv", delta: "，我看一下" });
  });

  // 玄女 bubble 累积出现 + streaming pulse
  await expect(page.getByTestId("msg-xuannv")).toContainText("好的，我看一下");
  await expect(page.getByTestId("msg-streaming")).toBeVisible();

  // EndOfTurn（agent_idle）→ pulse 消失
  await page.evaluate(() => {
    const ws = window.__FUXI_WS__.last as unknown as EventTarget;
    ws.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ type: "agent_idle", agent: "xuannv" }),
      }),
    );
  });
  await expect(page.getByTestId("msg-streaming")).toHaveCount(0);
});
