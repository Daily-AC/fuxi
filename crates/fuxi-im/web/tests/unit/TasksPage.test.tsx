import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { TasksPage } from "~/views/pages/TasksPage";
import { createMockApi } from "../mocks/api";
import type { NodesResponse, TasksOverview } from "~/types/api";
import { createEffect, type Component } from "solid-js";

afterEach(() => setApiOverride(null));

function setup(overview?: TasksOverview, nodes?: NodesResponse) {
  const api = createMockApi({ tasksOverview: overview, nodes });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={2}>
      <TasksPage />
    </ApiProvider>
  ));
}

const RUNNING_FIXTURE: TasksOverview = {
  running: [
    {
      id: "task-uuid-12345678",
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
          tokens: 1234,
          status: "busy",
          last_tool_call: { tool: "grep", args_summary: "server/api/v1.go" },
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

describe("TasksPage · v3 任务列表 (#N3' / #38)", () => {
  it("空 overview · 显示空态", async () => {
    const { getByTestId, queryByTestId, unmount } = setup({ running: [], completed: [] });
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("tasks-empty").textContent).toContain("还没有任务");
    expect(queryByTestId("tasks-running")).toBeNull();
    unmount();
  });

  it("进行中 · card header 含 title + 门客数 + duration", async () => {
    const { getByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    const head = getByTestId("task-card-head-task-uuid-12345678");
    expect(head.textContent).toContain("查 ERP API");
    expect(head.textContent).toContain("2 门客");
    expect(head.textContent).toContain("0:12");
    unmount();
  });

  it("v4 · 卡片不再渲染门客预览块（用户实测改：误点击 + role unknown 体验差）", async () => {
    const { queryByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    // 旧 v3 的 member 行 testid 不存在
    expect(queryByTestId("member-a-luban")).toBeNull();
    expect(queryByTestId("member-a-pusong")).toBeNull();
    unmount();
  });

  it("v4 · 卡片仅有 head button · tap 进 thread 页", async () => {
    const { getByTestId, queryByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("task-card-head-task-uuid-12345678")).toBeTruthy();
    // 卡片下方不再有 ul members
    expect(queryByTestId("member-a-luban")).toBeNull();
    unmount();
  });

  it("点 task card header · navPush({ kind: task, task_id, title })", async () => {
    const api = createMockApi({ tasksOverview: RUNNING_FIXTURE });
    setApiOverride(api);
    let pushed: { task_id: string | null; title: string | null } = {
      task_id: null,
      title: null,
    };
    const Probe: Component = () => {
      const { navRoute } = useApi();
      createEffect(() => {
        const r = navRoute();
        if (r?.kind === "task") {
          pushed = { task_id: r.task_id, title: r.title ?? null };
        }
      });
      return null;
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={2}>
        <TasksPage />
        <Probe />
      </ApiProvider>
    ));
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.click(getByTestId("task-card-head-task-uuid-12345678"));
    await new Promise((r) => setTimeout(r, 10));
    expect(pushed.task_id).toBe("task-uuid-12345678");
    expect(pushed.title).toBe("查 ERP API");
    unmount();
  });

  it("v4 · 卡片 header 仍含「N 门客」徽章（卡片不渲门客但保留计数）", async () => {
    const { getByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    const head = getByTestId("task-card-head-task-uuid-12345678");
    expect(head.textContent).toContain("2 门客");
    unmount();
  });

  it("v3 #44 · 已完成段直接平铺（无 sticky tail）+ 按 last_active_at 降序", async () => {
    const overview: TasksOverview = {
      running: [],
      completed: [
        // 故意把更早的放前面，验证 sort 后老的在底
        { id: "c-old", title: "整理日报", status: "completed", created_at: "", last_active_at: "2026-04-26T10:00:00Z", duration_ms: 200_000, members: [] },
        { id: "c-new", title: "升级 deps", status: "completed", created_at: "", last_active_at: "2026-04-26T11:00:00Z", duration_ms: 100_000, members: [] },
      ],
    };
    const { container, getByTestId, queryByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    // sticky tail 已删
    expect(queryByTestId("tasks-completed-tail")).toBeNull();
    // 已完成段直接展示卡片
    expect(getByTestId("task-card-c-old")).toBeTruthy();
    expect(getByTestId("task-card-c-new")).toBeTruthy();
    // 验排序：c-new 在前
    const cards = Array.from(
      container.querySelectorAll('[data-testid^="task-card-c-"]'),
    ).filter((el) => !el.getAttribute("data-testid")?.startsWith("task-card-head-"));
    expect(cards[0]?.getAttribute("data-testid")).toBe("task-card-c-new");
    expect(cards[1]?.getAttribute("data-testid")).toBe("task-card-c-old");
    unmount();
  });

  // v4 · @node 在线/离线 dim red 渲染移到详情页 ⋯ 菜单（TaskThreadPage）；
  // TasksPage 卡片不再渲门客预览，相关 v3 #59/#64 测试沿移到 TaskThreadPage.test.tsx

  // v4 · `member-node-*` 已移除；详情页 ⋯ 菜单内仍保留 @node 显示。

  it("进行中段按 last_active_at 降序 · 最近活动在上", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "old",
          title: "旧任务",
          status: "running",
          created_at: "",
          last_active_at: "2026-04-26T10:00:00Z",
          duration_ms: 0,
          members: [],
        },
        {
          id: "new",
          title: "新任务",
          status: "running",
          created_at: "",
          last_active_at: "2026-04-26T11:00:00Z",
          duration_ms: 0,
          members: [],
        },
      ],
      completed: [],
    };
    const { container, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    const cards = Array.from(
      container.querySelectorAll('[data-testid^="task-card-"]'),
    ).filter((el) => !el.getAttribute("data-testid")?.startsWith("task-card-head-"));
    expect(cards[0]?.getAttribute("data-testid")).toBe("task-card-new");
    expect(cards[1]?.getAttribute("data-testid")).toBe("task-card-old");
    unmount();
  });
});
