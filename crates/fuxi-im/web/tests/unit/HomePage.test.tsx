import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import {
  ApiProvider,
  setApiOverride,
  useApi,
  type ApiContextValue,
} from "~/components/ApiProvider";
import { MascotProvider } from "~/components/Mascot/MascotController";
import { HomePage } from "~/views/pages/HomePage";
import { createMockApi } from "../mocks/api";
import type { Component } from "solid-js";

afterEach(() => {
  setApiOverride(null);
});

// 捕获 api context，便于断言 setActiveTab 等导航 helper 的调用。
function setup(unreadCount = 3) {
  const api = createMockApi({
    tasksOverview: {
      running: [
        {
          id: "t-1",
          title: "跑 ERP",
          status: "running",
          created_at: "2026-06-04T00:00:00Z",
          last_active_at: "2026-06-04T00:00:00Z",
          duration_ms: 1000,
          members: [],
        },
        {
          id: "t-2",
          title: "查 API",
          status: "running",
          created_at: "2026-06-04T00:00:00Z",
          last_active_at: "2026-06-04T00:00:00Z",
          duration_ms: 1000,
          members: [],
        },
      ],
      completed: [],
    },
  });
  setApiOverride(api);

  let ctx!: ApiContextValue;
  const Capture: Component = () => {
    ctx = useApi();
    return <HomePage unreadCount={unreadCount} />;
  };

  const tools = render(() => (
    <ApiProvider initialAuth="in" initialTab={2}>
      <MascotProvider>
        <Capture />
      </MascotProvider>
    </ApiProvider>
  ));
  return { api, getCtx: () => ctx, ...tools };
}

describe("HomePage · 家·客厅主屏", () => {
  it("渲染 page-home + mascot + 问候含「以琳」", async () => {
    const { getByTestId, unmount } = setup();
    expect(getByTestId("page-home")).toBeTruthy();
    expect(getByTestId("home-mascot")).toBeTruthy();
    const greeting = getByTestId("home-greeting");
    expect(greeting.textContent).toContain("以琳");
    unmount();
  });

  it("状态行反映未读数「3」与门客在跑数", async () => {
    const { getByTestId, unmount } = setup(3);
    // resource 异步 resolve，等一拍
    await new Promise((r) => setTimeout(r, 30));
    const status = getByTestId("home-status");
    expect(status.textContent).toContain("3");
    expect(status.textContent).toContain("2"); // 两条 running task
    unmount();
  });

  it("4 个快捷入口瓦片全渲染", async () => {
    const { getByTestId, unmount } = setup();
    expect(getByTestId("home-action-chat")).toBeTruthy();
    expect(getByTestId("home-action-tasks")).toBeTruthy();
    expect(getByTestId("home-action-notifications")).toBeTruthy();
    expect(getByTestId("home-action-deliverables")).toBeTruthy();
    unmount();
  });

  it("点「找玄女聊」→ 切到聊天 tab(0)", async () => {
    const { getByTestId, getCtx, unmount } = setup();
    fireEvent.click(getByTestId("home-action-chat").querySelector("button")!);
    expect(getCtx().activeTab()).toBe(0);
    unmount();
  });

  it("点「看任务」→ 切到任务 tab(1)", async () => {
    const { getByTestId, getCtx, unmount } = setup();
    fireEvent.click(getByTestId("home-action-tasks").querySelector("button")!);
    expect(getCtx().activeTab()).toBe(1);
    unmount();
  });
});
