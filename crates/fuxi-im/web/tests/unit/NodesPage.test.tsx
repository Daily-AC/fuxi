import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { NodesPage } from "~/views/pages/NodesPage";
import { createMockApi } from "../mocks/api";
import type { NodesResponse } from "~/types/api";
import { createEffect, type Component } from "solid-js";

afterEach(() => setApiOverride(null));

function setup(nodes?: NodesResponse) {
  const api = createMockApi({ nodes });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={3} initialMoreSub="nodes">
      <NodesPage />
    </ApiProvider>
  ));
}

const HOME: NodesResponse["nodes"][number] = {
  node_id: "home",
  tags: ["home", "linux"],
  max_concurrency: 4,
  inflight_jobs: 1,
  heartbeat_lag_ms: 8230,
  online: true,
  registered_at: "2026-04-27T05:00:00Z",
  workers: [
    {
      agent_id: "a-luban",
      role: "luban",
      role_display: "鲁班",
      status: "busy",
      current_task_id: "t-erp",
      current_task_title: "查 ERP API",
    },
    {
      agent_id: "a-pusong",
      role: "pusong",
      role_display: "蒲松",
      status: "idle",
      current_task_id: null,
      current_task_title: null,
    },
  ],
};

const MAC: NodesResponse["nodes"][number] = {
  node_id: "mac-local",
  tags: ["local", "mac", "erp"],
  max_concurrency: 2,
  inflight_jobs: 0,
  heartbeat_lag_ms: 4120,
  online: true,
  registered_at: "2026-04-27T04:50:00Z",
  workers: [],
};

const OFFLINE: NodesResponse["nodes"][number] = {
  node_id: "old-node",
  tags: ["legacy"],
  max_concurrency: 1,
  inflight_jobs: 0,
  heartbeat_lag_ms: 99999,
  online: false,
  registered_at: null,
  workers: [],
};

describe("NodesPage v3 #58 · /api/nodes 真 topology", () => {
  it("空 nodes · 显空态 + 添加节点按钮", async () => {
    const { getByTestId, unmount } = setup({ nodes: [] });
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("nodes-empty").textContent).toContain("暂无节点");
    expect(getByTestId("nodes-add-btn")).toBeTruthy();
    unmount();
  });

  it("home 节点 · header 显节点名 + tags + inflight/max + workers 行", async () => {
    const { getByTestId, unmount } = setup({ nodes: [HOME] });
    await new Promise((r) => setTimeout(r, 30));
    const home = getByTestId("node-home");
    expect(home.textContent).toContain("home");
    expect(home.textContent).toContain("home");
    expect(home.textContent).toContain("linux");
    expect(home.textContent).toContain("1/4");
    expect(getByTestId("node-worker-a-luban").textContent).toContain("鲁班");
    expect(getByTestId("node-worker-a-luban").textContent).toContain("查 ERP API");
    expect(getByTestId("node-worker-a-pusong").textContent).toContain("蒲松");
    expect(getByTestId("node-worker-a-pusong").textContent).toContain("待命");
    unmount();
  });

  it("worker 行 busy > idle 排序", async () => {
    const overWithBoth: NodesResponse = {
      nodes: [
        {
          ...HOME,
          workers: [
            { ...HOME.workers[1]! }, // pusong idle
            { ...HOME.workers[0]! }, // luban busy
          ],
        },
      ],
    };
    const { container, unmount } = setup(overWithBoth);
    await new Promise((r) => setTimeout(r, 30));
    const rows = Array.from(container.querySelectorAll('[data-testid^="node-worker-"]'));
    expect(rows[0]?.getAttribute("data-testid")).toBe("node-worker-a-luban");
    expect(rows[1]?.getAttribute("data-testid")).toBe("node-worker-a-pusong");
    unmount();
  });

  it("空 workers 节点 · 显 \"当前无 worker 实例\"", async () => {
    const { getByTestId, unmount } = setup({ nodes: [MAC] });
    await new Promise((r) => setTimeout(r, 30));
    const mac = getByTestId("node-mac-local");
    expect(mac.textContent).toContain("当前无 worker 实例");
    unmount();
  });

  it("离线节点 · data-online=false + 整卡 muted（opacity 类）", async () => {
    const { getByTestId, unmount } = setup({ nodes: [OFFLINE] });
    await new Promise((r) => setTimeout(r, 30));
    const card = getByTestId("node-old-node");
    expect(card.getAttribute("data-online")).toBe("false");
    unmount();
  });

  it("online 节点排在 offline 前", async () => {
    const { container, unmount } = setup({ nodes: [OFFLINE, HOME] });
    await new Promise((r) => setTimeout(r, 30));
    const cards = Array.from(container.querySelectorAll('[data-testid^="node-"]')).filter(
      (el) => el.getAttribute("data-testid")?.startsWith("node-") && !el.getAttribute("data-testid")?.startsWith("node-worker-"),
    );
    expect(cards[0]?.getAttribute("data-testid")).toBe("node-home");
    expect(cards[1]?.getAttribute("data-testid")).toBe("node-old-node");
    unmount();
  });

  it("worker 行 tap · navPush 到任务 thread + 切到任务 tab", async () => {
    const api = createMockApi({ nodes: { nodes: [HOME] } });
    setApiOverride(api);
    let pushedTask: string | null = null;
    let switchedTab: number | null = null;
    const Probe: Component = () => {
      const { navRoute, activeTab } = useApi();
      createEffect(() => {
        const r = navRoute();
        if (r?.kind === "task") pushedTask = r.task_id;
        switchedTab = activeTab();
      });
      return null;
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3} initialMoreSub="nodes">
        <NodesPage />
        <Probe />
      </ApiProvider>
    ));
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.click(getByTestId("node-worker-a-luban").querySelector("button")!);
    await new Promise((r) => setTimeout(r, 10));
    expect(pushedTask).toBe("t-erp");
    expect(switchedTab).toBe(1);
    unmount();
  });

  it("idle worker (无 current_task_id) · button disabled 不可 tap", async () => {
    const { getByTestId, unmount } = setup({ nodes: [HOME] });
    await new Promise((r) => setTimeout(r, 30));
    const btn = getByTestId("node-worker-a-pusong").querySelector("button") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    unmount();
  });

  it("P2-A · idle worker 无 task = settled · data-settled=true + 等清理 badge", async () => {
    // 蒲松 fixture：idle + null current_task → "已完成等清理" 视觉折叠
    // 跑完一锤子任务等 IdleGc 600s 期间，PWA 不该让用户误以为该门客还活着接活
    const { getByTestId, queryByTestId, unmount } = setup({ nodes: [HOME] });
    await new Promise((r) => setTimeout(r, 30));
    const row = getByTestId("node-worker-a-pusong");
    expect(row.getAttribute("data-settled")).toBe("true");
    expect(queryByTestId("worker-settled-a-pusong")?.textContent).toContain("等清理");
    // 鲁班 busy 不该挂这个 badge
    const lubanRow = getByTestId("node-worker-a-luban");
    expect(lubanRow.getAttribute("data-settled")).toBe("false");
    expect(queryByTestId("worker-settled-a-luban")).toBeNull();
    unmount();
  });

  it("/api/nodes/stream 推 worker_heartbeat_state_changed → refetch + 实时刷 inflight 数", async () => {
    // 真因回归 · Bug B：NodesPage 旧版只 fetchNodes 一次无 WS 订阅，inflight 数
    // 永不动。修后必须订阅 nodes-stream，每条 push refetch /api/nodes 拿最新数。
    const initial: NodesResponse = {
      nodes: [{ ...HOME, inflight_jobs: 0 }],
    };
    const api = createMockApi({ nodes: initial });
    setApiOverride(api);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3} initialMoreSub="nodes">
        <NodesPage />
      </ApiProvider>
    ));
    // 等首屏 fetch
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("node-home").textContent).toContain("0/4");

    // controller 端 home.inflight 变了 → 后端 /api/nodes 下次返新数
    api.state.nodes = { nodes: [{ ...HOME, inflight_jobs: 2 }] };
    // ws push 一条节点级事件——NodesPage 应触发 refetch
    api.pushNodesStream({
      meta: {
        id: "ev-1",
        at: "2026-04-27T05:00:01Z",
        agent: null,
        task: null,
      },
      kind: {
        type: "worker_heartbeat_state_changed",
        node_id: "home",
        inflight_count: 2,
        status: "alive",
      },
    });
    // 等 refetch 落地
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("node-home").textContent).toContain("2/4");
    unmount();
  });

  it("添加节点按钮 · 弹 modal 显 install command", async () => {
    const { getByTestId, queryByTestId, unmount } = setup({ nodes: [] });
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.click(getByTestId("nodes-add-btn"));
    expect(getByTestId("nodes-add-modal")).toBeTruthy();
    expect(getByTestId("nodes-install-cmd").textContent).toContain("setup-local-worker.sh");
    fireEvent.click(getByTestId("nodes-add-close"));
    expect(queryByTestId("nodes-add-modal")).toBeNull();
    unmount();
  });
});
