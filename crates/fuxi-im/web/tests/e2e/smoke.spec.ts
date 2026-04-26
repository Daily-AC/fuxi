import { test, expect } from "@playwright/test";

// v2 阶段 1：视觉骨架 + 空态。
// 验证：登入后看到 Header（任务/玄女/节点）+ Conversation 空态 + Composer。

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    interface PendingFetch {
      input: string;
    }
    const fetchCalls: PendingFetch[] = [];
    (window as unknown as { __FUXI_FETCH__: PendingFetch[] }).__FUXI_FETCH__ = fetchCalls;

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

test("登入后 main shell 出现，Header 三 tap target + Composer + 空态", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId("header-tasks")).toBeVisible();
  await expect(page.getByTestId("header-center")).toContainText("玄女");
  await expect(page.getByTestId("header-nodes")).toBeVisible();
  await expect(page.getByTestId("conversation-empty")).toContainText("玄女在线");
  await expect(page.getByTestId("conversation-empty")).toContainText("跟她说点啥");
  await expect(page.getByTestId("composer-input")).toBeVisible();
  await expect(page.getByTestId("composer-send")).toBeVisible();
});

test("composer 输入后发送按钮变 active（accent fill）", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  const send = page.getByTestId("composer-send");
  await expect(send).toBeDisabled();
  await page.getByTestId("composer-input").fill("hi");
  await expect(send).toBeEnabled();
});
