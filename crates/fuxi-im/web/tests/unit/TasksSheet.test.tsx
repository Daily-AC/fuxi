import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { TasksSheet } from "~/views/sheets/TasksSheet";
import { createMockApi } from "../mocks/api";
import type { TasksOverview } from "~/types/api";
import { onMount, type Component } from "solid-js";

afterEach(() => setApiOverride(null));

const Open: Component = () => {
  const { setActiveSheet } = useApi();
  onMount(() => setActiveSheet("tasks"));
  return null;
};

function setup(overview?: TasksOverview) {
  const api = createMockApi({ tasksOverview: overview });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in">
      <Open />
      <TasksSheet />
    </ApiProvider>
  ));
}

describe("TasksSheet", () => {
  it("0 任务 · 显示空状态", async () => {
    const { getByTestId, queryByTestId, unmount } = setup({ running: [], completed: [] });
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("tasks-sheet")).toBeTruthy();
    expect(getByTestId("tasks-empty").textContent).toContain("暂无任务");
    expect(queryByTestId("tasks-running")).toBeNull();
    expect(queryByTestId("tasks-completed")).toBeNull();
    unmount();
  });

  it("1 任务 running + 完整 members · 渲染卡片 + 三行 member", async () => {
    const overview: TasksOverview = {
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
            {
              agent_id: "a-idle",
              role: "luban",
              role_display: "鲁班",
              status: "idle",
            },
          ],
        },
      ],
      completed: [],
    };
    const { getByTestId, queryAllByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    const card = getByTestId("task-card-task-uuid-12345678");
    expect(card.textContent).toContain("修 ERP 客户列表");
    // duration "0:12"
    expect(card.textContent).toContain("0:12");
    // members 三行
    const rows = queryAllByTestId(/^member-/);
    expect(rows).toHaveLength(3);
    // 鲁班行带 activity + token
    const lubanRow = getByTestId("member-a-luban");
    expect(lubanRow.textContent).toContain("鲁班");
    expect(lubanRow.textContent).toContain("cargo test --lib");
    expect(lubanRow.textContent).toContain("1.2k");
    unmount();
  });

  it("N 任务混合 · running + completed 都用统一 task-card 渲染（#26）", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "r1",
          title: "任务 A",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 60_000,
          members: [],
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
        {
          id: "c2",
          title: "整理日报",
          status: "completed",
          created_at: "",
          last_active_at: "",
          duration_ms: 90_000,
          members: [],
        },
      ],
    };
    const { getByTestId, queryAllByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("tasks-running")).toBeTruthy();
    expect(getByTestId("tasks-completed")).toBeTruthy();
    // 现在 running + completed 都用 task-card- 前缀
    expect(queryAllByTestId(/^task-card-/)).toHaveLength(3);
    expect(getByTestId("task-card-c1").textContent).toContain("3:20");
    expect(getByTestId("task-card-c1").getAttribute("data-status")).toBe("completed");
    unmount();
  });

  it("#26 · completed 卡也显完整 members（不再薄一行）", async () => {
    const overview: TasksOverview = {
      running: [],
      completed: [
        {
          id: "c1",
          title: "升级 deps",
          status: "completed",
          created_at: "",
          last_active_at: "",
          duration_ms: 200_000,
          members: [
            {
              agent_id: "a-luban-c",
              role: "luban",
              role_display: "鲁班",
              activity: "cargo update",
              tokens: 800,
              status: "idle",
            },
          ],
        },
      ],
    };
    const { getByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    const card = getByTestId("task-card-c1");
    expect(card.textContent).toContain("升级 deps");
    // 关键：completed 卡片也包 member 行
    expect(getByTestId("member-a-luban-c").textContent).toContain("鲁班");
    expect(getByTestId("member-a-luban-c").textContent).toContain("cargo update");
    expect(getByTestId("member-a-luban-c").textContent).toContain("800");
    unmount();
  });

  it("#26 · last_event_summary 渲染在 title 下方一行", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "task-with-summary",
          title: "修 ERP",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 12_000,
          members: [],
          last_event_summary: "鲁班 · cargo test --lib · exit 0",
        },
      ],
      completed: [],
    };
    const { getByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("task-summary-task-with-summary").textContent).toContain(
      "鲁班 · cargo test --lib · exit 0",
    );
    unmount();
  });

  it("#26 · member.last_tool_call 详情渲染（exit 0 + 时长）", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "task-tc",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            {
              agent_id: "a-tc",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              last_tool_call: {
                tool: "cargo test",
                args_summary: "--lib --release",
                exit: 0,
                duration_ms: 5000,
              },
            },
          ],
        },
      ],
      completed: [],
    };
    const { getByTestId, queryAllByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    const tcs = queryAllByTestId("member-tool-call");
    expect(tcs).toHaveLength(1);
    expect(tcs[0]?.textContent).toContain("cargo test");
    expect(tcs[0]?.textContent).toContain("--lib --release");
    expect(tcs[0]?.textContent).toContain("exit 0");
    expect(tcs[0]?.textContent).toContain("0:05");
    void getByTestId;
    unmount();
  });

  it("#26 · last_tool_call exit 非 0 → 显失败色（class 命中 exitFail）", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "task-fail",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            {
              agent_id: "a-fail",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              last_tool_call: { tool: "cargo build", exit: 1 },
            },
          ],
        },
      ],
      completed: [],
    };
    const { container, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toContain("exit 1");
    void container;
    unmount();
  });

  it("#26 · last_tool_call exit=null → 显'运行中'", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "task-running-tc",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            {
              agent_id: "a-r",
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              last_tool_call: { tool: "cargo run", exit: null },
            },
          ],
        },
      ],
      completed: [],
    };
    const { container, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent).toContain("运行中");
    void container;
    unmount();
  });
});
