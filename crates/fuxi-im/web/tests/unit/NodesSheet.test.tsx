import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { NodesSheet, aggregateHomeNode } from "~/views/sheets/NodesSheet";
import { createMockApi } from "../mocks/api";
import type { TasksOverview } from "~/types/api";
import { onMount, type Component } from "solid-js";

afterEach(() => setApiOverride(null));

const Open: Component = () => {
  const { setActiveSheet } = useApi();
  onMount(() => setActiveSheet("nodes"));
  return null;
};

function setup(overview?: TasksOverview) {
  const api = createMockApi({ tasksOverview: overview });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in">
      <Open />
      <NodesSheet />
    </ApiProvider>
  ));
}

describe("aggregateHomeNode 聚合逻辑", () => {
  it("空 overview · home offline 0 agents", () => {
    const n = aggregateHomeNode({ running: [], completed: [] });
    expect(n.online).toBe(false);
    expect(n.agents).toHaveLength(0);
  });

  it("undefined overview · home offline", () => {
    const n = aggregateHomeNode(undefined);
    expect(n.online).toBe(false);
  });

  it("同 agent 多 task · tokens 累加 + status 取最忙", () => {
    const n = aggregateHomeNode({
      running: [
        {
          id: "t1",
          title: "",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            {
              agent_id: "a1",
              role: "luban",
              role_display: "鲁班",
              tokens: 100,
              status: "thinking",
            },
          ],
        },
        {
          id: "t2",
          title: "",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            {
              agent_id: "a1",
              role: "luban",
              role_display: "鲁班",
              tokens: 50,
              status: "busy", // 比 thinking 忙 → 取这个
            },
          ],
        },
      ],
      completed: [],
    });
    expect(n.agents).toHaveLength(1);
    expect(n.agents[0]?.tokens).toBe(150);
    expect(n.agents[0]?.status).toBe("busy");
  });

  it("agents 排序 · busy > thinking > idle，再按 role_display 中文排", () => {
    const n = aggregateHomeNode({
      running: [
        {
          id: "t",
          title: "",
          status: "running",
          created_at: "",
          last_active_at: "",
          duration_ms: 0,
          members: [
            { agent_id: "a-i", role: "luban", role_display: "鲁班-i", status: "idle" },
            { agent_id: "a-b", role: "pusong", role_display: "蒲松-b", status: "busy" },
            { agent_id: "a-t", role: "luban", role_display: "鲁班-t", status: "thinking" },
          ],
        },
      ],
      completed: [],
    });
    expect(n.agents.map((a) => a.agent_id)).toEqual(["a-b", "a-t", "a-i"]);
  });
});

describe("NodesSheet 渲染", () => {
  it("空 overview · 显示 home offline + 没有活跃 agent", async () => {
    const { getByTestId, unmount } = setup({ running: [], completed: [] });
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("nodes-sheet")).toBeTruthy();
    expect(getByTestId("nodes-offline").textContent).toContain("离线");
    expect(getByTestId("node-home").textContent).toContain("当前没有活跃 agent");
    unmount();
  });

  it("有活跃 agent · 显示 online + agent 列表", async () => {
    const { getByTestId, queryAllByTestId, unmount } = setup({
      running: [
        {
          id: "t1",
          title: "",
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
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("nodes-online").textContent).toContain("在线");
    const rows = queryAllByTestId(/^node-agent-/);
    expect(rows).toHaveLength(2);
    expect(getByTestId("node-agent-a-luban").textContent).toContain("鲁班");
    expect(getByTestId("node-agent-a-luban").textContent).toContain("运行中");
    expect(getByTestId("node-agent-a-luban").textContent).toContain("1.5k");
    expect(getByTestId("node-agent-a-pusong").textContent).toContain("空闲");
    unmount();
  });
});
