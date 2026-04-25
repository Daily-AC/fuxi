import { describe, expect, it } from "vitest";
import { MemoryRouter, Route } from "@solidjs/router";
import { render } from "@solidjs/testing-library";
import { TaskCard } from "~/components/TaskCard";
import type { TaskCard as TaskCardType } from "~/types/events";

const sample: TaskCardType = {
  id: "task-1234567890ab",
  title: "修 ERP 客户列表分页",
  status: "running",
  created_at: "2026-04-26T11:00:00Z",
  updated_at: "2026-04-26T11:55:00Z",
  agent: "cc-7b3fdeadbeef",
  parent: null,
  summary: "已经拿到 PR diff，正在跑测试。",
};

describe("TaskCard", () => {
  it("渲染标题 / 状态 / agent id 短化", () => {
    const { container, unmount } = render(() => (
      <MemoryRouter>
        <Route path="*" component={() => <TaskCard task={sample} />} />
      </MemoryRouter>
    )) as { container: HTMLElement; unmount: () => void };
    expect(container.textContent ?? "").toContain("修 ERP 客户列表分页");
    expect(container.textContent ?? "").toContain("进行中");
    expect(container.textContent ?? "").toContain("cc-7-beef");
    unmount();
  });

  it("data-status 上挂状态供测试 / 样式钩子使用", () => {
    const { container, unmount } = render(() => (
      <MemoryRouter>
        <Route
          path="*"
          component={() => <TaskCard task={{ ...sample, status: "done" }} />}
        />
      </MemoryRouter>
    )) as { container: HTMLElement; unmount: () => void };
    const card = container.querySelector('[data-testid^="task-card-"]');
    expect(card?.getAttribute("data-status")).toBe("done");
    unmount();
  });
});
