import { test, expect } from "@playwright/test";

// Bug 14B 守护：用户在任务 tab 上，应该 0 个 conv WS 连接被建立。
// 即使从 #/conv 切回 #/，原 controller 应被 dispose，5s 内不再建新连接。

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    interface PendingFetch {
      input: string;
    }
    const fetchCalls: PendingFetch[] = [];
    const wsUrls: string[] = [];
    (window as unknown as { __FUXI_FETCH__: PendingFetch[] }).__FUXI_FETCH__ = fetchCalls;
    (window as unknown as { __FUXI_WS_URLS__: string[] }).__FUXI_WS_URLS__ = wsUrls;

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
        wsUrls.push(url);
        // 不主动 close —— 走"正常服务端响应"路径
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

test("任务 tab 上 5 秒内不打开 /api/conv WS（Bug 14B）", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("tasks-view")).toBeVisible();
  // 给足时间：onMount + auth probe 都跑完 + 一段空闲
  await page.waitForTimeout(5_000);
  const convWsCount = await page.evaluate(() => {
    const urls = (window as unknown as { __FUXI_WS_URLS__: string[] }).__FUXI_WS_URLS__;
    return urls.filter((u) => u.includes("/api/conv")).length;
  });
  expect(convWsCount).toBe(0);
});

test("从 #/conv 切回 #/ 后，5 秒内不再建新 /api/conv WS（防 dispose 漏）", async ({ page }) => {
  await page.goto("/#/conv");
  await expect(page.getByTestId("conv-view")).toBeVisible();
  // 等 conv WS 至少建 1 个
  await expect
    .poll(async () =>
      page.evaluate(() =>
        (window as unknown as { __FUXI_WS_URLS__: string[] }).__FUXI_WS_URLS__.filter((u) =>
          u.includes("/api/conv"),
        ).length,
      ),
    )
    .toBeGreaterThan(0);
  const beforeSwitch = await page.evaluate(() =>
    (window as unknown as { __FUXI_WS_URLS__: string[] }).__FUXI_WS_URLS__.filter((u) =>
      u.includes("/api/conv"),
    ).length,
  );
  // 切回任务 tab → ConvView unmount → dispose
  await page.getByTestId("nav-任务").click();
  await expect(page.getByTestId("tasks-view")).toBeVisible();
  await page.waitForTimeout(5_000);
  const afterSwitch = await page.evaluate(() =>
    (window as unknown as { __FUXI_WS_URLS__: string[] }).__FUXI_WS_URLS__.filter((u) =>
      u.includes("/api/conv"),
    ).length,
  );
  expect(afterSwitch).toBe(beforeSwitch);
});

test("空数组 tasks → 显示'当前没有任务'空状态，不卡 loader（Bug 14A）", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("tasks-view")).toBeVisible();
  await expect(page.getByText("当前没有任务")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId("tasks-loading")).toHaveCount(0);
});
