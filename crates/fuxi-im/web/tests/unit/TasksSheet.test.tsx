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

  it("N 任务混合 · running + completed 都渲染", async () => {
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
    expect(queryAllByTestId(/^task-completed-/)).toHaveLength(2);
    expect(getByTestId("task-completed-c1").textContent).toContain("3:20");
    unmount();
  });
});
