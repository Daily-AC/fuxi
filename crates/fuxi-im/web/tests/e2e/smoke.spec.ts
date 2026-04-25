import { test, expect } from "@playwright/test";

// 跑前 vite preview 已 build。这套 e2e 用页面注入 mock：
// 在加载前把 window.__FUXI_MOCK__ 喂进去，prod-build 的入口会 detect 并切换。
// 当前我们让 page 直接 mock fetch + WS，足以验证三个 view 加载 + 顶部输入条工作。

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    // 在加载前 patch fetch + WebSocket
    const tasks = [
      {
        id: "task-aaaaaaaaaaaa",
        title: "修 ERP 客户列表分页",
        status: "running",
        created_at: "2026-04-26T11:00:00Z",
        updated_at: "2026-04-26T11:55:00Z",
        agent: "cc-7b3fdeadbeef",
        parent: null,
        summary: "拿到 PR diff，正在跑测试。",
      },
      {
        id: "task-bbbbbbbbbbbb",
        title: "整理本周日报",
        status: "done",
        created_at: "2026-04-25T20:00:00Z",
        updated_at: "2026-04-25T22:00:00Z",
        agent: "codex-12345678",
        parent: null,
        summary: "已经写完。",
      },
    ];

    interface PendingFetch {
      input: string;
      init?: RequestInit;
    }
    const calls: PendingFetch[] = [];
    (window as unknown as { __FUXI_FETCH__: PendingFetch[] }).__FUXI_FETCH__ = calls;

    const json = (data: unknown, status = 200): Response =>
      new Response(JSON.stringify(data), {
        status,
        headers: { "content-type": "application/json" },
      });

    const realFetch = window.fetch.bind(window);
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      calls.push({ input: url, init });
      if (url.startsWith("/api/tasks") && (!init || init.method === undefined)) {
        if (url.includes("/events")) {
          return json({ events: [], next_cursor: null });
        }
        return json({ tasks });
      }
      if (url === "/api/intervene") {
        return json({ ok: true });
      }
      if (url === "/api/dispatch") {
        return json({ task_id: "t-new" });
      }
      if (url === "/api/push/vapid-pub") {
        return json({ public_key: "stub" });
      }
      if (url === "/api/push/subscribe") {
        return json({ ok: true });
      }
      return realFetch(input as RequestInfo, init);
    };

    class FakeWS extends EventTarget {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSING = 2;
      static CLOSED = 3;
      readyState = 0;
      url: string;
      constructor(url: string) {
        super();
        this.url = url;
        setTimeout(() => {
          this.readyState = 1;
          this.dispatchEvent(new Event("open"));
          if (url.includes("/api/conv")) {
            const ev = {
              type: "agent_text",
              agent: "xuannv",
              text: "我在听呢",
              ts: "2026-04-26T12:00:00Z",
            };
            this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(ev) }));
          }
        }, 50);
      }
      send(): void {}
      close(): void {
        this.readyState = 3;
        this.dispatchEvent(new Event("close"));
      }
    }
    (window as unknown as { WebSocket: typeof FakeWS }).WebSocket = FakeWS;
  });
});

test("加载主屏，渲染任务卡片", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("tasks-view")).toBeVisible();
  await expect(page.getByTestId("task-card-task-aaaaaaaaaaaa")).toBeVisible();
  await expect(page.getByText("修 ERP 客户列表分页")).toBeVisible();
  await expect(page.getByText("进行中")).toBeVisible();
});

test("点任务卡片进入单 task 视图", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("task-card-task-aaaaaaaaaaaa").click();
  await expect(page.getByTestId("task-view")).toBeVisible();
  // task-view 内部应该显示截短的 task id
  await expect(page.getByTestId("task-view").getByText("#task-aaa")).toBeVisible();
});

test("玄女对话视图 + 流式渲染收到一些字", async ({ page }) => {
  await page.goto("/#/conv");
  await expect(page.getByTestId("conv-view")).toBeVisible();
  await expect(page.getByText("我在听呢")).toBeVisible({ timeout: 5_000 });
});

test("e2e: 顶部输入条派活 → 后端收到 intervene", async ({ page }) => {
  await page.goto("/");
  const input = page.getByTestId("xuannv-input");
  await input.fill("帮我看一下 ERP 任务进度");
  await input.press("Enter");
  // 验证 fetch 被调用到 /api/intervene
  await expect
    .poll(async () => {
      return await page.evaluate(() => {
        const calls = (
          window as unknown as { __FUXI_FETCH__: Array<{ input: string }> }
        ).__FUXI_FETCH__;
        return calls.filter((c) => c.input === "/api/intervene").length;
      });
    })
    .toBeGreaterThan(0);
  // 输入框应该被清空
  await expect(input).toHaveValue("");
});
