import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { TasksPage } from "~/views/pages/TasksPage";
import { createMockApi } from "../mocks/api";
import type { TasksOverview } from "~/types/api";
import { createEffect, type Component } from "solid-js";

afterEach(() => setApiOverride(null));

function setup(overview?: TasksOverview) {
  const api = createMockApi({ tasksOverview: overview });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={1}>
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
    expect(getByTestId("tasks-empty").textContent).toContain("暂无任务");
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

  it("member 行 · role 加粗 + 副文本（last_tool_call.tool + args）· 不再有 chev ›", async () => {
    const { getByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    const lubanRow = getByTestId("member-a-luban");
    expect(lubanRow.textContent).toContain("鲁班");
    expect(lubanRow.textContent).toContain("grep server/api/v1.go");
    expect(lubanRow.textContent).not.toContain("›");
    unmount();
  });

  it("member 行 inspection-only · v3 不可 tap（无 button）", async () => {
    const { getByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    const row = getByTestId("member-a-luban");
    expect(row.tagName.toLowerCase()).toBe("li");
    expect(row.querySelector("button")).toBeNull();
    unmount();
  });

  it("member 行无 last_tool_call · 显状态 fallback（待命/思考中）", async () => {
    const { getByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("member-a-pusong").textContent).toContain("待命");
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
      <ApiProvider initialAuth="in" initialTab={1}>
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

  it("v3 · members 始终展开（无 expand/collapse 切换）", async () => {
    const { getByTestId, unmount } = setup(RUNNING_FIXTURE);
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("member-a-luban")).toBeTruthy();
    expect(getByTestId("member-a-pusong")).toBeTruthy();
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

  it("v3 #59 · member 行有 node_id 时显 @node 标识", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "t-mix",
          title: "混合",
          status: "running",
          created_at: "",
          last_active_at: "2026-04-26T11:00:00Z",
          duration_ms: 5_000,
          members: [
            {
              agent_id: "a-luban-mac",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              last_tool_call: { tool: "grep", args_summary: "src/main.go" },
              node_id: "mac-local",
            },
            {
              agent_id: "a-pusong-home",
              role: "pusong",
              role_display: "蒲松",
              status: "idle",
              node_id: "home",
            },
          ],
        },
      ],
      completed: [],
    };
    const { getByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    const lubanRow = getByTestId("member-a-luban-mac");
    expect(lubanRow.textContent).toContain("@mac-local");
    expect(getByTestId("member-node-a-luban-mac").textContent).toBe("@mac-local");
    const pusongRow = getByTestId("member-a-pusong-home");
    expect(pusongRow.textContent).toContain("@home");
    unmount();
  });

  it("v3 #59 · member 无 node_id 时不显 @node", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "t-no-node",
          title: "无节点信息",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            {
              agent_id: "a-no-node",
              role: "luban",
              role_display: "鲁班",
              status: "idle",
            },
          ],
        },
      ],
      completed: [],
    };
    const { queryByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("member-node-a-no-node")).toBeNull();
    unmount();
  });

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
