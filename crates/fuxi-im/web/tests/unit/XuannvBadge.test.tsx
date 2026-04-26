import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render, waitFor } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { XuannvPage } from "~/views/pages/XuannvPage";
import { createMockApi } from "../mocks/api";
import type { TasksOverview } from "~/types/api";
import type { Component } from "solid-js";

afterEach(() => {
  setApiOverride(null);
});

function setup(overview: TasksOverview | undefined) {
  const api = createMockApi({ tasksOverview: overview });
  setApiOverride(api);
  let getTab: () => number = () => -1;
  const TabProbe: Component = () => {
    const { activeTab } = useApi();
    getTab = () => activeTab();
    return null;
  };
  const tools = render(() => (
    <ApiProvider initialAuth="in" initialTab={0}>
      <XuannvPage />
      <TabProbe />
    </ApiProvider>
  ));
  return { api, getTab: () => getTab(), ...tools };
}

describe("XuannvPage · sticky badge \"✓ 抄送 N 门客\" (#N4)", () => {
  it("running.length=0 · badge 不显（隐藏空态）", async () => {
    const { queryByTestId, unmount } = setup({ running: [], completed: [] });
    await new Promise((r) => setTimeout(r, 50));
    expect(queryByTestId("cc-badge")).toBeNull();
    unmount();
  });

  it("undefined overview · badge 不显（fetch 失败兜底）", async () => {
    const { queryByTestId, unmount } = setup(undefined);
    await new Promise((r) => setTimeout(r, 50));
    expect(queryByTestId("cc-badge")).toBeNull();
    unmount();
  });

  it("running.length=2 · badge 显 \"抄送 2 门客\" + ✓ icon", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "t1",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
        {
          id: "t2",
          title: "y",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
      ],
      completed: [],
    };
    const { findByTestId, unmount } = setup(overview);
    const badge = await findByTestId("cc-badge");
    expect(badge.textContent).toContain("抄送");
    expect(badge.textContent).toContain("2");
    expect(badge.textContent).toContain("门客");
    expect(badge.textContent).toContain("✓");
    unmount();
  });

  it("tap badge · setActiveTab(1) 切到任务 tab (v3)", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "t1",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
      ],
      completed: [],
    };
    const { findByTestId, getTab, unmount } = setup(overview);
    const badge = await findByTestId("cc-badge");
    expect(getTab()).toBe(0);
    fireEvent.click(badge);
    await waitFor(() => expect(getTab()).toBe(1));
    unmount();
  });

  it("completed 任务不计数 · 只 completed 时不显 badge", async () => {
    const overview: TasksOverview = {
      running: [],
      completed: [
        {
          id: "c1",
          title: "done",
          status: "completed",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
      ],
    };
    const { queryByTestId, unmount } = setup(overview);
    await new Promise((r) => setTimeout(r, 50));
    expect(queryByTestId("cc-badge")).toBeNull();
    unmount();
  });

  it("aria-label 包含 N · 屏幕阅读器友好", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "t1",
          title: "x",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
        {
          id: "t2",
          title: "y",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
        {
          id: "t3",
          title: "z",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [],
        },
      ],
      completed: [],
    };
    const { findByTestId, unmount } = setup(overview);
    const badge = await findByTestId("cc-badge");
    expect(badge.getAttribute("aria-label")).toContain("3");
    unmount();
  });
});
