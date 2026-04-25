import { test, expect } from "@playwright/test";

// 鉴权 e2e：未登入态 + 登入流程 + 错误码 + PIN 降级。
// fetch mock 在 addInitScript 阶段控制：第一次 GET /api/tasks 401 → 显 LoginView；
// POST /api/auth/login 按 page.evaluate 切换的 token 决定 200/401/503/429。

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    interface PendingFetch {
      input: string;
      init?: RequestInit;
    }
    const calls: PendingFetch[] = [];
    (window as unknown as { __FUXI_FETCH__: PendingFetch[] }).__FUXI_FETCH__ = calls;
    const ctrl = (window as unknown as {
      __FUXI_AUTH_CTRL__: { loggedIn: boolean; loginStatus: number; pairStatus: number };
    });
    ctrl.__FUXI_AUTH_CTRL__ = { loggedIn: false, loginStatus: 200, pairStatus: 200 };

    const json = (data: unknown, status = 200): Response =>
      new Response(JSON.stringify(data), {
        status,
        headers: { "content-type": "application/json" },
      });

    const realFetch = window.fetch.bind(window);
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      calls.push({ input: url, init });
      const c = ctrl.__FUXI_AUTH_CTRL__;

      if (url === "/api/auth/login") {
        if (c.loginStatus === 200) {
          c.loggedIn = true;
          return json({ device_id: "dev-e2e" });
        }
        return new Response("nope", { status: c.loginStatus });
      }
      if (url === "/api/auth/pair") {
        if (c.pairStatus === 200) {
          c.loggedIn = true;
          return json({ device_id: "dev-pair-e2e" });
        }
        return new Response("nope", { status: c.pairStatus });
      }

      // 鉴权 gate：未登入时一切 /api/* 401（但 auth/login & auth/pair 上面已捕获）
      if (!c.loggedIn) {
        return new Response("unauthorized", { status: 401 });
      }

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
      constructor() {
        super();
        setTimeout(() => {
          this.readyState = 1;
          this.dispatchEvent(new Event("open"));
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

test("未登入 → 显示 LoginView 不显示主 shell", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("login-view")).toBeVisible();
  await expect(page.getByTestId("view-slot")).toHaveCount(0);
});

test("主密码 200 → 进入主屏，看到任务列表 view", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("login-password").fill("正确密码");
  await page.getByTestId("login-device-name").fill("e2e iPhone");
  await page.getByTestId("login-submit").click();
  await expect(page.getByTestId("tasks-view")).toBeVisible({ timeout: 5_000 });
  // login-view 应该消失
  await expect(page.getByTestId("login-view")).toHaveCount(0);
});

test("主密码 401 → 显示'密码错'不进主屏", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    (window as unknown as { __FUXI_AUTH_CTRL__: { loginStatus: number } }).__FUXI_AUTH_CTRL__.loginStatus = 401;
  });
  await page.getByTestId("login-password").fill("错密码");
  await page.getByTestId("login-submit").click();
  await expect(page.getByTestId("login-error")).toBeVisible();
  await expect(page.getByTestId("login-error")).toContainText("密码错");
  await expect(page.getByTestId("login-view")).toBeVisible();
});

test("主密码 503 → 引导文案 fuxi im set-password", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    (window as unknown as { __FUXI_AUTH_CTRL__: { loginStatus: number } }).__FUXI_AUTH_CTRL__.loginStatus = 503;
  });
  await page.getByTestId("login-password").fill("x");
  await page.getByTestId("login-submit").click();
  await expect(page.getByTestId("login-error")).toContainText("fuxi im set-password");
});

test("点'忘了' → PIN 降级，配对 200 → 进主屏", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("login-mode-switch").click();
  await page.getByTestId("login-pin").fill("482917");
  await page.getByTestId("login-device-name").fill("配对设备");
  await page.getByTestId("login-submit").click();
  await expect(page.getByTestId("tasks-view")).toBeVisible({ timeout: 5_000 });
});
