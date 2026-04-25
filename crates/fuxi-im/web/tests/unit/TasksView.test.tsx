import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { MemoryRouter, Route } from "@solidjs/router";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { TasksView } from "~/views/TasksView";
import { createMockApi } from "../mocks/api";

function renderTasksView(api = createMockApi()) {
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in">
      <MemoryRouter>
        <Route path="*" component={TasksView} />
      </MemoryRouter>
    </ApiProvider>
  )) as { container: HTMLElement; unmount: () => void };
}

describe("TasksView", () => {
  it("空数组 → 显示'当前没有任务'空状态，不卡在 loader（Bug 14A）", async () => {
    const api = createMockApi({ tasks: [] });
    const { container, unmount } = renderTasksView(api);
    // 等 createResource 解析 + Solid 调度
    await new Promise((r) => setTimeout(r, 30));
    expect(container.querySelector('[data-testid="tasks-loading"]')).toBeNull();
    expect(container.textContent ?? "").toContain("当前没有任务");
    setApiOverride(null);
    unmount();
  });

  it("有任务 → 渲染卡片，不显示空状态文案", async () => {
    const api = createMockApi({
      tasks: [
        {
          id: "task-1234",
          title: "测试任务",
          status: "running",
          created_at: "2026-04-26T11:00:00Z",
          updated_at: "2026-04-26T11:55:00Z",
          agent: "cc-abc",
          parent: null,
          summary: null,
        },
      ],
    });
    const { container, unmount } = renderTasksView(api);
    await new Promise((r) => setTimeout(r, 30));
    expect(container.textContent ?? "").toContain("测试任务");
    expect(container.textContent ?? "").not.toContain("当前没有任务");
    setApiOverride(null);
    unmount();
  });
});
