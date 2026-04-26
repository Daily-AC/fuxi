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
      if (url === "/api/push/vapid-pub") return json({ public_key: "stub" });
      if (url === "/api/push/subscribe") return json({ ok: true });
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

test("登入后 main shell · BottomTabBar 3 tab + 玄女默认 + Composer 空态 (v3)", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible({ timeout: 5_000 });
  // tab bar 渲 3 项
  await expect(page.getByTestId("tab-bar")).toBeVisible();
  await expect(page.getByTestId("tab-xuannv")).toBeVisible();
  await expect(page.getByTestId("tab-tasks")).toBeVisible();
  await expect(page.getByTestId("tab-nodes")).toBeVisible();
  // 玄女 默认 active
  await expect(page.getByTestId("tab-xuannv")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
  await expect(page.getByTestId("conversation-empty")).toContainText("玄女在线");
  await expect(page.getByTestId("mention-editor")).toBeVisible();
});

test("BottomTabBar · 点 tab-nodes 切节点页 / 点 tab-tasks 切任务页 (v3)", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-nodes").click();
  await expect(page.getByTestId("tab-nodes")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-nodes")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("tab-tasks")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("page-tasks")).toBeVisible();
});

test("composer 输入后发送按钮变 active", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  const send = page.getByTestId("mention-send");
  await expect(send).toBeDisabled();
  await page.getByTestId("mention-editor").fill("hi");
  await expect(send).toBeEnabled();
});

test("发送消息 → user bubble + intervene 调用 + WS 流式收到玄女回复", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
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

  // pager dot 切到任务页（C 方案两级行卡片，#N2）
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("page-tasks")).toBeVisible();
  await expect(page.getByTestId("task-card-task-uuid-12345678")).toContainText("修 ERP 客户列表");
  await expect(page.getByTestId("member-a-luban")).toContainText("鲁班");
  await expect(page.getByTestId("member-a-luban")).toContainText("cargo test --lib");
  // C 方案 completed 默认折叠 sticky tail，需展开才看到 c1 卡
  await page.getByTestId("tasks-completed-tail").click();
  await expect(page.getByTestId("task-card-c1")).toContainText("升级 deps");
  // dot 1 切回玄女
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
});

test("Pager · 切节点页 · 聚合 home 节点 + agents 列表（dot 0）", async ({ page }) => {
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
            {
              agent_id: "a-luban",
              role: "luban",
              role_display: "鲁班",
              tokens: 1500,
              status: "busy",
            },
            {
              agent_id: "a-pusong",
              role: "pusong",
              role_display: "蒲松",
              status: "idle",
            },
          ],
        },
      ],
      completed: [],
    };
  });
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();

  await page.getByTestId("tab-nodes").click();
  await expect(page.getByTestId("page-nodes")).toBeVisible();
  await expect(page.getByTestId("node-home")).toContainText("home");
  await expect(page.getByTestId("node-agent-a-luban")).toContainText("鲁班");
  await expect(page.getByTestId("node-agent-a-luban")).toContainText("1.5k");
});

test("Pager · 多次切换 · 节点→任务→玄女 都正常", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("main-shell")).toBeVisible();
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("page-tasks")).toBeVisible();
  await page.getByTestId("tab-nodes").click();
  await expect(page.getByTestId("page-nodes")).toBeVisible();
  await page.getByTestId("tab-xuannv").click();
  await expect(page.getByTestId("page-xuannv")).toBeVisible();
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
  // 默认在玄女 tab
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

// ===== #N3' / #38 任务卡片 tap → push 任务 thread (v3 stub) =====

test("#N3' · 任务卡片 tap → push Layer 2 thread placeholder", async ({ page }) => {
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
  await page.getByTestId("tab-tasks").click();
  await expect(page.getByTestId("page-tasks")).toBeVisible();

  // 点任务卡 header → push thread placeholder
  await page.getByTestId("task-card-head-task-erp").click();
  await expect(page.getByTestId("task-thread-stub")).toBeVisible();
  await expect(page.getByTestId("task-thread-stub")).toContainText("查 ERP API");

  // ‹ 列表 pop 回任务列表
  await page.getByTestId("task-thread-back").click();
  await expect(page.getByTestId("task-thread-stub")).toHaveCount(0);
  await expect(page.getByTestId("page-tasks")).toBeVisible();
});

// ===== #N3 v2 私聊页 e2e 已删 (v3 supersede 后 per-worker push 概念去除) =====
// 私聊页源码 src/views/pages/WorkerPage.tsx 暂留作 v2.x 备用（per spec），但 e2e 流不再走。
// 任务 thread (#39/#N4') 实装后会有新的 task-thread-* 一族 e2e 替代这里。
// 单测 tests/unit/WorkerPage.test.tsx 也保留作 reducer/render 单元覆盖。

test("#N2 · completed 卡折叠 sticky tail · 展开后显 members + tool 副文本", async ({ page }) => {
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

  // C 方案 · completed 默认折叠 sticky tail
  await expect(page.getByTestId("tasks-completed-tail")).toContainText("已完成 · 1 条");
  // 展开
  await page.getByTestId("tasks-completed-tail").click();
  await expect(page.getByTestId("task-card-c-dense")).toBeVisible();
  // 完整 members 显（spec C 方案 completed 也显 members）
  await expect(page.getByTestId("member-a-c-luban")).toContainText("鲁班");
  // 副文本 = last_tool_call.tool（C 方案不再单独显 tokens / exit 码 / 时长，改 #N3 私聊页才显详）
  await expect(page.getByTestId("member-a-c-luban")).toContainText("cargo update");
});
