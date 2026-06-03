import { test, expect } from "@playwright/test";

// v2 阶段 1+2：视觉骨架 + 玄女/user message + intervene + WS 流式。

declare global {
  interface Window {
    __FUXI_FETCH__: Array<{ input: string; body?: string }>;
    __FUXI_WS__: { last: WebSocketLike | null };
    __FUXI_WORKER_WS__: { last: WebSocketLike | null; url: string | null };
    __FUXI_HISTORY__?: unknown[];
    __FUXI_TASKS_OVERVIEW__?: unknown;
    __FUXI_INTERVENE_NEXT_STATUS__?: number;
    __FUXI_NODES__?: unknown;
  }
}

interface WebSocketLike {
  dispatchEvent(e: Event): boolean;
  url: string;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const fetchCalls: Array<{ input: string; body?: string }> = [];
    window.__FUXI_FETCH__ = fetchCalls;
    window.__FUXI_WS__ = { last: null };
    window.__FUXI_WORKER_WS__ = { last: null, url: null };

    const json = (data: unknown, status = 200): Response =>
      new Response(JSON.stringify(data), {
        status,
        headers: { "content-type": "application/json" },
      });

    const realFetch = window.fetch.bind(window);
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      const body = typeof init?.body === "string" ? init.body : undefined;
      fetchCalls.push({ input: url, body });
      if (url.startsWith("/api/tasks")) {
        if (url.includes("/events")) return json({ events: [], next_cursor: null });
        // fetchTasks(rootOnly=true) 用作 auth probe，走 ?root=1，返 v1 shape
        if (url.includes("root=1")) return json({ tasks: [] });
        // fetchTasksOverview 用作阶段 4 任务 sheet · 检查 window.__FUXI_TASKS_OVERVIEW__
        const overview = (
          window as unknown as { __FUXI_TASKS_OVERVIEW__?: unknown }
        ).__FUXI_TASKS_OVERVIEW__;
        return json(overview ?? { running: [], completed: [] });
      }
      if (url === "/api/intervene") {
        const next = (window as unknown as { __FUXI_INTERVENE_NEXT_STATUS__?: number })
          .__FUXI_INTERVENE_NEXT_STATUS__;
        if (typeof next === "number" && next !== 200) {
          (window as unknown as { __FUXI_INTERVENE_NEXT_STATUS__?: number })
            .__FUXI_INTERVENE_NEXT_STATUS__ = undefined;
          return json({ error: "busy" }, next);
        }
        return json({ ok: true });
      }
      if (url.startsWith("/api/workers/")) {
        // /api/workers/:agent_id/events  · 私聊页历史
        return json({ events: [], next_cursor: null });
      }
      if (url === "/api/nodes") {
        const inj = (window as unknown as { __FUXI_NODES__?: unknown }).__FUXI_NODES__;
        return json(inj ?? { nodes: [] });
      }
      if (url === "/api/push/vapid-pub") return json({ public_key: "stub" });
      if (url === "/api/push/subscribe") return json({ ok: true });
      // Phase 1 · XuannvPage 拉 topics（header 副标题）+ projects（@ 候选追加段）。
      // 玄女 tab 不再是默认页 → 切过去才 mount 这些 fetch。mock 必须兜底，否则
      // 落到 realFetch（preview 无后端返 index.html）→ .json() throw → composer 子树坏，
      // @ autocomplete popup 不弹。
      if (url.startsWith("/api/topics")) return json({ topics: [] });
      if (url.startsWith("/api/projects")) return json({ projects: [] });
      if (url.startsWith("/api/notifications")) return json({ unread_count: 0 });
      if (url.startsWith("/api/conv/messages")) {
        const inj = (window as unknown as { __FUXI_HISTORY__?: unknown[] }).__FUXI_HISTORY__;
        return json({ messages: inj ?? [], next_before: null });
      }
      return realFetch(input as RequestInfo, init);
    };

    // XHR mock for /api/upload —— Composer 用 XHR 拿 progress
    class FakeXHR {
      static UNSENT = 0;
      static OPENED = 1;
      static HEADERS_RECEIVED = 2;
      static LOADING = 3;
      static DONE = 4;
      readyState = 0;
      status = 0;
      statusText = "";
      response: unknown = null;
      responseType = "";
      withCredentials = false;
      upload = new EventTarget();
      private listeners = new Map<string, Array<(e: any) => void>>();
      private url = "";
      private method = "";
      open(method: string, url: string): void {
        this.method = method;
        this.url = url;
      }
      addEventListener(t: string, fn: (e: any) => void): void {
        const a = this.listeners.get(t) ?? [];
        a.push(fn);
        this.listeners.set(t, a);
      }
      send(_body?: unknown): void {
        if (this.url === "/api/upload" && this.method === "POST") {
          // 模拟一次进度 + load
          (this.upload as EventTarget).dispatchEvent(
            new ProgressEvent("progress", { loaded: 100, total: 100, lengthComputable: true }),
          );
          this.status = 200;
          this.statusText = "OK";
          this.response = {
            id: `up-e2e-${Math.random().toString(36).slice(2, 6)}`,
            name: "fixture.txt",
            mime: "text/plain",
            bytes: 100,
            sha256: "sha",
          };
          this.readyState = 4;
          for (const fn of this.listeners.get("load") ?? []) fn(new Event("load"));
        } else {
          // fallback
          this.status = 0;
          for (const fn of this.listeners.get("error") ?? []) fn(new Event("error"));
        }
      }
    }
    (window as unknown as { XMLHttpRequest: unknown }).XMLHttpRequest =
      FakeXHR as unknown as typeof XMLHttpRequest;

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
        // 私聊页 socket 单独 expose · /api/workers/:id/conv
        if (url.includes("/api/workers/")) {
          window.__FUXI_WORKER_WS__.last = this;
          window.__FUXI_WORKER_WS__.url = url;
        }
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

test("登入后 main shell · BottomTabBar 4 tab + 家默认 + 切玄女后 Composer 空态 (daimeng 重构 4-tab)", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible({ timeout: 5_000 });
  // tab bar 渲 4 项：家 / 聊天(玄女) / 任务 / 更多（无通知一级 tab）
  await expect(page.getByTestId("tab-bar")).toBeVisible();
  await expect(page.getByTestId("tab-home")).toBeVisible();
  await expect(page.getByTestId("tab-xuannv")).toBeVisible();
  await expect(page.getByTestId("tab-tasks")).toBeVisible();
  await expect(page.getByTestId("tab-more")).toBeVisible();
  await expect(page.getByTestId("tab-notifications")).toHaveCount(0);
  // 家 默认 active
  await expect(page.getByTestId("tab-home")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-home")).toBeVisible();
  // 切到聊天 tab → 玄女会话空态 + composer
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("tab-xuannv")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await expect(page.getByTestId("conversation-empty")).toContainText("玄女在线");
  await expect(page.getByTestId("mention-editor")).toBeVisible();
});

test("BottomTabBar · 点 tab-more 进 hub · 点 tile-nodes 沉到节点页 / 返回回 hub", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-more").click();
  await expect(page.getByTestId("tab-more")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-more")).toBeVisible();
  await page.getByTestId("more-tile-nodes").click();
  await expect(page.getByTestId("page-nodes")).toBeVisible();
  // 返回 hub
  await page.getByTestId("more-sub-back").click();
  await expect(page.getByTestId("page-more")).toBeVisible();
});

test("BottomTabBar · 点 tab-tasks 切任务页 (v1-session17 任务 tab 仍在 1)", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("tab-tasks")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-tasks")).toBeVisible();
});

test("composer 输入后发送按钮变 active", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  const send = page.getByTestId("mention-send");
  await expect(send).toBeDisabled();
  await page.getByTestId("mention-editor").fill("hi");
  await expect(send).toBeEnabled();
});

test("发送消息 → user bubble + intervene 调用 + WS 流式收到玄女回复", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  // 等 ws open
  await page.waitForFunction(() => window.__FUXI_WS__.last !== null);

  // 输入并发送
  await page.getByTestId("mention-editor").fill("hi 玄女");
  await page.getByTestId("mention-send").click();

  // optimistic：user bubble 立刻出现
  await expect(page.getByTestId("msg-user")).toContainText("hi 玄女");

  // intervene fetch 被调用
  await expect
    .poll(async () =>
      page.evaluate(() => window.__FUXI_FETCH__.filter((c) => c.input === "/api/intervene").length),
    )
    .toBeGreaterThan(0);

  // 服务端推 agent_text_delta 三段（嵌套 wire format · { meta, kind }）
  await page.evaluate(() => {
    const ws = window.__FUXI_WS__.last as unknown as EventTarget;
    const send = (kind: unknown): void => {
      const ev = {
        meta: {
          id: `id-${Math.random().toString(36).slice(2, 8)}`,
          at: new Date().toISOString(),
          session: null,
          agent: "f0d0f576-fa97-4a0c-9c25-test",
          task: null,
        },
        kind,
      };
      ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(ev) }));
    };
    send({ type: "agent_text_delta", delta: "好" });
    send({ type: "agent_text_delta", delta: "的" });
    send({ type: "agent_text_delta", delta: "，我看一下" });
  });

  // 玄女 bubble 累积出现 + streaming pulse
  await expect(page.getByTestId("msg-xuannv")).toContainText("好的，我看一下");
  await expect(page.getByTestId("msg-streaming")).toBeVisible();

  // EndOfTurn（agent_idle）→ pulse 消失（嵌套 wire format）
  await page.evaluate(() => {
    const ws = window.__FUXI_WS__.last as unknown as EventTarget;
    const ev = {
      meta: {
        id: "id-end",
        at: new Date().toISOString(),
        session: null,
        agent: "f0d0f576-fa97-4a0c-9c25-test",
        task: null,
      },
      kind: { type: "agent_idle" },
    };
    ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(ev) }));
  });
  await expect(page.getByTestId("msg-streaming")).toHaveCount(0);
});

test("玄女回 markdown 长文 → bubble 渲染 strong + code + link", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await page.waitForFunction(() => window.__FUXI_WS__.last !== null);

  await page.evaluate(() => {
    const ws = window.__FUXI_WS__.last as unknown as EventTarget;
    const md = [
      "**好的**，我看了一下：",
      "",
      "- 用 `cargo test --lib`",
      "- 文档：[link](https://example.com/docs)",
      "",
      "```",
      "let x = 1;",
      "```",
    ].join("\n");
    const ev = {
      meta: {
        id: "id-md",
        at: new Date().toISOString(),
        session: null,
        agent: "f0d0f576-fa97-4a0c-9c25-test",
        task: null,
      },
      kind: { type: "agent_responded", text: md },
    };
    ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(ev) }));
  });

  const bubble = page.getByTestId("msg-xuannv-text");
  // strong 渲染
  await expect(bubble.locator("strong")).toContainText("好的");
  // inline code
  await expect(bubble.locator("code").first()).toContainText("cargo test");
  // link 强制 _blank + rel
  const link = bubble.locator("a");
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
  // fenced code block
  await expect(bubble.locator("pre code")).toContainText("let x = 1;");
});

test("XSS · 玄女回 <script> 不执行（被 sanitize）", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await page.waitForFunction(() => window.__FUXI_WS__.last !== null);

  let attacked = false;
  page.on("dialog", async (d) => {
    attacked = true;
    await d.dismiss();
  });

  await page.evaluate(() => {
    const ws = window.__FUXI_WS__.last as unknown as EventTarget;
    const ev = {
      meta: {
        id: "id-xss",
        at: new Date().toISOString(),
        session: null,
        agent: "f0d0f576-fa97-4a0c-9c25-test",
        task: null,
      },
      kind: { type: "agent_responded", text: "<script>alert(1)</script>合法文本" },
    };
    ws.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(ev) }));
  });

  await expect(page.getByTestId("msg-xuannv-text")).toContainText("合法文本");
  // 等一段确保 script 没执行
  await page.waitForTimeout(200);
  expect(attacked).toBe(false);
  // bubble 内不应有 script tag
  const scriptCount = await page.getByTestId("msg-xuannv-text").locator("script").count();
  expect(scriptCount).toBe(0);
});

// v3 玄女 tab 用 MentionComposer，不再支持文件附件 attachments（spec §C 没列）。
// 老 Composer + AttachmentChip 单测仍保留作 v2.x 备用，但 e2e 流（file-input → upload → intervene
// 带 attachments）下线。如果用户实测又要附件，重开 task 加进 MentionComposer。

test("历史预加载 · mock 5 条 stored message → 看到 5 条进入", async ({ page }) => {
  await page.addInitScript(() => {
    window.__FUXI_HISTORY__ = [
      { id: "h1", conv_id: "xuannv", role: "user", kind: "text", content: "之前的提问 A", ts: "2026-04-26T10:00:00Z" },
      { id: "h2", conv_id: "xuannv", role: "xuannv", kind: "text", content: "之前的回答 A", ts: "2026-04-26T10:00:30Z" },
      { id: "h3", conv_id: "xuannv", role: "user", kind: "text", content: "之前的提问 B", ts: "2026-04-26T10:01:00Z" },
      { id: "h4", conv_id: "xuannv", role: "xuannv", kind: "text", content: "之前的回答 B", ts: "2026-04-26T10:01:30Z" },
      { id: "h5", conv_id: "xuannv", role: "user", kind: "text", content: "之前的提问 C", ts: "2026-04-26T10:02:00Z" },
    ];
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await expect(page.getByTestId("msg-user").first()).toContainText("之前的提问 A");
  await expect(page.locator('[data-testid="msg-user"]')).toHaveCount(3);
  await expect(page.locator('[data-testid="msg-xuannv"]')).toHaveCount(2);
});

// ===== 阶段 4 · 任务 sheet + 节点 sheet =====

test("Pager · 切任务页 · 看到 mock 数据 + 切回玄女", async ({ page }) => {
  await page.addInitScript(() => {
    window.__FUXI_TASKS_OVERVIEW__ = {
      running: [
        {
          id: "task-uuid-12345678",
          title: "修 ERP 客户列表",
          status: "running",
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 12_000,
          members: [
            {
              agent_id: "a-luban",
              role: "luban",
              role_display: "鲁班",
              activity: "cargo test --lib",
              tokens: 1234,
              status: "busy",
            },
            {
              agent_id: "a-pusong",
              role: "pusong",
              role_display: "蒲松",
              activity: "Read git log",
              status: "thinking",
            },
          ],
        },
      ],
      completed: [
        {
          id: "c1",
          title: "升级 deps",
          status: "completed",
          created_at: "",
          last_active_at: "",
          duration_ms: 200_000,
          members: [],
        },
      ],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();

  // 任务 tab
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("page-tasks")).toBeVisible();
  await expect(page.getByTestId("task-card-task-uuid-12345678")).toContainText("修 ERP 客户列表");
  // RESKIN：门客活动折叠进卡片副标题（lead 门客 · 活动），不再单独 member-<id> 元素
  await expect(page.getByTestId("task-card-task-uuid-12345678")).toContainText("鲁班");
  await expect(page.getByTestId("task-card-task-uuid-12345678")).toContainText("cargo test --lib");
  // v3 #44：已完成段直接平铺，无 sticky tail
  await expect(page.getByTestId("task-card-c1")).toContainText("升级 deps");
  // 切回玄女
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
});

test("#58 · 节点 tab 切真 /api/nodes · 显 home + mac-local + workers", async ({ page }) => {
  await page.addInitScript(() => {
    window.__FUXI_NODES__ = {
      nodes: [
        {
          node_id: "home",
          tags: ["home", "linux"],
          max_concurrency: 4,
          inflight_jobs: 1,
          heartbeat_lag_ms: 8230,
          online: true,
          registered_at: "2026-04-27T05:00:00Z",
          workers: [
            {
              agent_id: "a-luban",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              current_task_id: "t-erp",
              current_task_title: "查 ERP API",
            },
          ],
        },
        {
          node_id: "mac-local",
          tags: ["local", "mac", "erp"],
          max_concurrency: 2,
          inflight_jobs: 0,
          heartbeat_lag_ms: 4120,
          online: true,
          registered_at: "2026-04-27T04:50:00Z",
          workers: [],
        },
      ],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-more").click();
  await page.getByTestId("more-tile-nodes").click();
  await expect(page.getByTestId("page-nodes")).toBeVisible();
  // home 节点：显 worker
  const home = page.getByTestId("node-home");
  await expect(home).toContainText("home");
  await expect(home).toContainText("1/4");
  await expect(home).toContainText("linux");
  await expect(page.getByTestId("node-worker-a-luban")).toContainText("鲁班");
  await expect(page.getByTestId("node-worker-a-luban")).toContainText("查 ERP API");
  // mac-local：在线但无 worker
  const mac = page.getByTestId("node-mac-local");
  await expect(mac).toContainText("mac-local");
  await expect(mac).toContainText("0/2");
  await expect(mac).toContainText("当前无 worker 实例");
});

test("#58 · 节点空 · 显空态提示加节点", async ({ page }) => {
  await page.addInitScript(() => {
    window.__FUXI_NODES__ = { nodes: [] };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-more").click();
  await page.getByTestId("more-tile-nodes").click();
  await expect(page.getByTestId("nodes-empty")).toContainText("暂无节点");
});

test("#58 · 添加节点按钮 · 弹 modal 显 install command", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-more").click();
  await page.getByTestId("more-tile-nodes").click();
  await page.getByTestId("nodes-add-btn").click();
  const modal = page.getByTestId("nodes-add-modal");
  await expect(modal).toBeVisible();
  await expect(page.getByTestId("nodes-install-cmd")).toContainText("setup-local-worker.sh");
  // 关 modal
  await page.getByTestId("nodes-add-close").click();
  await expect(modal).toHaveCount(0);
});

test("Pager · 多次切换 · 节点→任务→玄女 都正常", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("page-tasks")).toBeVisible();
  await page.getByTestId("tab-more").click();
  await page.getByTestId("more-tile-nodes").click();
  await expect(page.getByTestId("page-nodes")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
});

// ===== #46 composer + 附件入口（玄女 tab 测，任务 thread 共用同 MentionComposer 路径）=====

test("#46 · 玄女 tab + 选 PNG → attach chip → 发 → intervene 带 attachments", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await expect(page.getByTestId("composer-attach-btn")).toBeVisible();

  // 选文件
  await page.getByTestId("composer-file-input").setInputFiles({
    name: "screenshot.png",
    mimeType: "image/png",
    buffer: Buffer.from("fake-bytes"),
  });
  // attach chip 出现
  await expect(page.locator('[data-testid^="composer-attach-c-"][data-status]')).toHaveCount(1);

  // 输文字 + 发
  await page.getByTestId("mention-editor").fill("看看这张");
  await page.getByTestId("mention-send").click();

  // intervene body 含 attachments + text
  await expect.poll(() =>
    page.evaluate(() => {
      const c = window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene");
      return c?.body ?? "";
    }),
  ).toContain('"attachments"');
  const body = await page.evaluate(() =>
    window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene")?.body ?? "",
  );
  expect(body).toContain('"text":"看看这张"');
});

// ===== v2 #N4 sticky badge e2e 已删（v3 #40 删 badge）=====

// ===== #N5' / #40 玄女 tab @ chip composer =====

test("#N5' · 玄女 tab @ chip · 输 @ 弹 autocomplete + 选 → chip + 发 intervene 带 target+mentions", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.__FUXI_TASKS_OVERVIEW__ = {
      running: [
        {
          id: "t1",
          title: "查 ERP",
          status: "running",
          created_at: "",
          last_active_at: "2026-04-26T11:00:00Z",
          duration_ms: 0,
          members: [
            {
              agent_id: "a-luban",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
            },
          ],
        },
      ],
      completed: [],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  // 切到聊天 tab（家是默认）
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();

  // 输 @ 触发 autocomplete
  await page.getByTestId("mention-editor").fill("@");
  await expect(page.getByTestId("mention-popup")).toBeVisible();
  await expect(page.getByTestId("mention-item-a-luban")).toContainText("鲁班");

  // 选鲁班 → chip
  await page.getByTestId("mention-item-a-luban").click();
  await expect(page.getByTestId("mention-chip-a-luban")).toBeVisible();

  // 输入正文 + 发
  await page.getByTestId("mention-editor").fill(" 帮我看 v1.go");
  await page.getByTestId("mention-send").click();

  // 验 intervene body 带 target=a-luban + mentions=[a-luban]
  await expect
    .poll(() =>
      page.evaluate(() => {
        const c = window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene");
        return c?.body ?? "";
      }),
    )
    .toContain('"target":"a-luban"');
  const body = await page.evaluate(() =>
    window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene")?.body ?? "",
  );
  expect(body).toContain('"mentions":["a-luban"]');
});

test("#N5' · 玄女 tab 无 chip · intervene 不带 target（backend 走玄女默认）", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await page.getByTestId("mention-editor").fill("你好玄女");
  await page.getByTestId("mention-send").click();
  await expect.poll(() =>
    page.evaluate(() => window.__FUXI_FETCH__.filter((c) => c.input === "/api/intervene").length),
  ).toBeGreaterThan(0);
  const body = await page.evaluate(() =>
    window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene")?.body ?? "",
  );
  // 无 chip 时 target/mentions 字段都不下发
  expect(body).not.toContain('"target"');
  expect(body).not.toContain('"mentions"');
  expect(body).toContain('"text":"你好玄女"');
});

// 老 #N3' placeholder e2e 已被下面 #N4' 替代（真 TaskThreadPage 渲染替代 stub）

// ===== #N4' / #39 任务 thread = 群聊 mix 全成员 =====

test("#N4' · 任务卡 push 后渲染真 TaskThreadPage（顶栏+banner+thread+composer）", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.__FUXI_TASKS_OVERVIEW__ = {
      running: [
        {
          id: "task-erp",
          title: "查 ERP API",
          status: "running",
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 12_000,
          members: [
            { agent_id: "a-xn", role: "xuannv", role_display: "玄女", status: "busy" },
            {
              agent_id: "a-luban",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              last_tool_call: { tool: "Bash", args_summary: "grep" },
            },
          ],
        },
      ],
      completed: [],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await page.getByTestId("task-card-head-task-erp").click();

  // 真 TaskThreadPage（不是 placeholder）
  await expect(page.getByTestId("task-thread-page")).toBeVisible();
  await expect(page.getByTestId("task-thread-page")).toHaveAttribute("data-task-id", "task-erp");
  await expect(page.getByTestId("task-thread-page")).toContainText("查 ERP API");

  // 任务 banner
  await expect(page.getByTestId("task-banner")).toContainText("进行中");
  await expect(page.getByTestId("task-banner")).toContainText("0:12");
  await expect(page.getByTestId("task-banner")).toContainText("2 门客");
  await expect(page.getByTestId("task-banner-members")).toContainText("玄女");
  await expect(page.getByTestId("task-banner-members")).toContainText("鲁班");

  // composer 默认 placeholder "对玄女说..."
  await expect(
    page.getByTestId("task-thread-page").getByTestId("mention-editor"),
  ).toHaveAttribute("placeholder", /玄女/);

  // ‹ 列表 pop
  await page.getByTestId("task-thread-back").click();
  await expect(page.getByTestId("task-thread-page")).toHaveCount(0);
  await expect(page.getByTestId("page-tasks")).toBeVisible();
});

test("#N4' · @ 选鲁班 → intervene 带 task_id+target+mentions", async ({ page }) => {
  await page.addInitScript(() => {
    window.__FUXI_TASKS_OVERVIEW__ = {
      running: [
        {
          id: "t1",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            { agent_id: "a-xn", role: "xuannv", role_display: "玄女", status: "busy" },
            { agent_id: "a-luban-2", role: "luban", role_display: "鲁班", status: "busy" },
          ],
        },
      ],
      completed: [],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await page.getByTestId("task-card-head-t1").click();
  await expect(page.getByTestId("task-thread-page")).toBeVisible();

  const editor = page.getByTestId("task-thread-page").getByTestId("mention-editor");
  await editor.fill("@lu");
  await page.getByTestId("mention-item-a-luban-2").click();
  await editor.fill(" 帮我看");
  await page.getByTestId("task-thread-page").getByTestId("mention-send").click();

  await expect.poll(() =>
    page.evaluate(() => {
      const c = window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene");
      return c?.body ?? "";
    }),
  ).toContain('"target":"a-luban-2"');
  const body = await page.evaluate(() =>
    window.__FUXI_FETCH__.find((x) => x.input === "/api/intervene")?.body ?? "",
  );
  expect(body).toContain('"task_id":"t1"');
  expect(body).toContain('"mentions":["a-luban-2"]');
});

// ===== #N3 v2 私聊页 e2e 已删 (v3 supersede 后 per-worker push 概念去除) =====
// 私聊页源码 src/views/pages/WorkerPage.tsx 暂留作 v2.x 备用（per spec），但 e2e 流不再走。
// 单测 tests/unit/WorkerPage.test.tsx 也保留作 reducer/render 单元覆盖。

test("#44 · completed 段直接平铺（无 sticky tail）+ 显 members + tool 副文本", async ({ page }) => {
  await page.addInitScript(() => {
    window.__FUXI_TASKS_OVERVIEW__ = {
      running: [],
      completed: [
        {
          id: "c-dense",
          title: "升级 deps",
          status: "completed",
          created_at: "",
          last_active_at: "",
          duration_ms: 200_000,
          last_event_summary: "鲁班 · cargo update · exit 0",
          members: [
            {
              agent_id: "a-c-luban",
              role: "luban",
              role_display: "鲁班",
              tokens: 800,
              status: "idle",
              last_tool_call: {
                tool: "cargo update",
                exit: 0,
                duration_ms: 8000,
              },
            },
          ],
        },
      ],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("page-tasks")).toBeVisible();

  // v3 #44：sticky tail 已删；卡片直接见底
  await expect(page.getByTestId("tasks-completed-tail")).toHaveCount(0);
  await expect(page.getByTestId("task-card-c-dense")).toBeVisible();
  // RESKIN：门客活动折叠进卡片副标题（lead 门客 · 工具），不再单独 member-<id> 元素
  await expect(page.getByTestId("task-card-c-dense")).toContainText("鲁班");
  await expect(page.getByTestId("task-card-c-dense")).toContainText("cargo update");
});
