import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { TaskThreadPage } from "~/views/pages/TaskThreadPage";
import { Toast } from "~/components/Toast";
import { dismissToast } from "~/lib/toast";
import { createMockApi } from "../mocks/api";
import type { ServerEvent } from "~/types/events";
import type { NodesResponse, TasksOverview } from "~/types/api";

const TASK_ID = "task-erp";
const XUANNV = "agent-xuannv";
const LUBAN = "agent-luban";
const PUSONG = "agent-pusong";

const TASK_OVERVIEW: TasksOverview = {
  running: [
    {
      id: TASK_ID,
      title: "查 ERP API",
      status: "running",
      created_at: "2026-04-26T11:00:00Z",
      last_active_at: "2026-04-26T11:12:00Z",
      duration_ms: 12_000,
      members: [
        { agent_id: XUANNV, role: "xuannv", role_display: "玄女", status: "busy" },
        {
          agent_id: LUBAN,
          role: "luban",
          role_display: "鲁班",
          status: "busy",
          last_tool_call: { tool: "Bash", args_summary: "grep server" },
        },
        { agent_id: PUSONG, role: "pusong", role_display: "蒲松", status: "idle" },
      ],
    },
  ],
  completed: [],
};

afterEach(() => {
  setApiOverride(null);
  dismissToast();
});

function ev(
  kind: ServerEvent["kind"],
  meta: Partial<ServerEvent["meta"]> = {},
): ServerEvent {
  return {
    meta: {
      id: meta.id ?? `id-${Math.random().toString(36).slice(2, 8)}`,
      at: meta.at ?? "2026-04-26T12:00:00Z",
      session: meta.session ?? null,
      agent: meta.agent === undefined ? null : meta.agent,
      task: meta.task ?? TASK_ID,
    },
    kind,
  };
}

function setup(opts?: {
  overview?: TasksOverview;
  taskEvents?: ServerEvent[];
  interveneSeq?: number[];
  nodes?: NodesResponse;
}) {
  const api = createMockApi({
    tasksOverview: opts?.overview ?? TASK_OVERVIEW,
    events: opts?.taskEvents ? { [TASK_ID]: opts.taskEvents } : {},
    interveneSeq: opts?.interveneSeq,
    nodes: opts?.nodes,
  });
  setApiOverride(api);
  const tools = render(() => (
    <ApiProvider initialAuth="in" initialTab={3}>
      <TaskThreadPage task_id={TASK_ID} fallback_title="查 ERP API" />
      <Toast />
    </ApiProvider>
  ));
  return { api, ...tools };
}

describe("TaskThreadPage · v3 #N4' / #39", () => {
  it("顶栏 + ‹ 列表 + title 渲染", async () => {
    const { getByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 50));
    const back = getByTestId("task-thread-back");
    expect(back.textContent).toContain("列表");
    const page = getByTestId("task-thread-page");
    expect(page.getAttribute("data-task-id")).toBe(TASK_ID);
    expect(page.textContent).toContain("查 ERP API");
    unmount();
  });

  it("任务 banner · 状态 + duration + 成员数 + 第二行成员列表", async () => {
    const { findByTestId, unmount } = setup();
    const banner = await findByTestId("task-banner");
    expect(banner.textContent).toContain("进行中");
    expect(banner.textContent).toContain("0:12");
    expect(banner.textContent).toContain("3 门客");
    const members = await findByTestId("task-banner-members");
    expect(members.textContent).toContain("玄女");
    expect(members.textContent).toContain("鲁班");
    expect(members.textContent).toContain("Bash");
    unmount();
  });

  it("v3 #59 · 任务 banner 第二行成员含 @node 标识", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: TASK_ID,
          title: "混合节点任务",
          status: "running",
          created_at: "",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 5000,
          members: [
            {
              agent_id: LUBAN,
              role: "luban",
              role_display: "鲁班",
              status: "busy",
              last_tool_call: { tool: "grep" },
              node_id: "mac-local",
            },
            {
              agent_id: PUSONG,
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
    // 节点 mock：mac-local + home 都在线 → @node 走 muted gray 路径（#64 引入）
    const nodes: NodesResponse = {
      nodes: [
        { node_id: "mac-local", tags: [], max_concurrency: 8, inflight_jobs: 0, heartbeat_lag_ms: 100, online: true, registered_at: null, workers: [] },
        { node_id: "home", tags: [], max_concurrency: 4, inflight_jobs: 0, heartbeat_lag_ms: 100, online: true, registered_at: null, workers: [] },
      ],
    };
    const { findByTestId, unmount } = setup({ overview, nodes });
    const members = await findByTestId("task-banner-members");
    expect(members.textContent).toContain("鲁班");
    expect(members.textContent).toContain("@mac-local");
    expect(members.textContent).toContain("蒲松");
    expect(members.textContent).toContain("@home");
    unmount();
  });

  it("v3 #64 · banner @node 节点离线 · testid banner-node-offline-<id>", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: TASK_ID,
          title: "stale 节点任务",
          status: "running",
          created_at: "",
          last_active_at: "2026-04-26T11:00:00Z",
          duration_ms: 0,
          members: [
            {
              agent_id: LUBAN,
              role: "luban",
              role_display: "鲁班",
              status: "idle",
              node_id: "mac-local",
            },
          ],
        },
      ],
      completed: [],
    };
    // mac-local heartbeat 超时 → online=false
    const nodes: NodesResponse = {
      nodes: [
        { node_id: "mac-local", tags: [], max_concurrency: 8, inflight_jobs: 0, heartbeat_lag_ms: 99999, online: false, registered_at: null, workers: [] },
      ],
    };
    const { findByTestId, queryByTestId, unmount } = setup({ overview, nodes });
    await findByTestId("task-banner-members");
    expect(queryByTestId(`banner-node-offline-${LUBAN}`)?.textContent).toBe("@mac-local");
    expect(queryByTestId(`banner-node-${LUBAN}`)).toBeNull();
    unmount();
  });

  it("WS 玄女 agent_responded · 渲染 xuannv bubble", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 50));
    api.pushTask(TASK_ID, ev({ type: "agent_responded", text: "好的派给鲁班" }, { agent: XUANNV }));
    await new Promise((r) => setTimeout(r, 30));
    const xn = queryAllByTestId("msg-xuannv");
    expect(xn).toHaveLength(1);
    expect(xn[0]?.textContent).toContain("好的派给鲁班");
    unmount();
  });

  it("WS worker agent_responded · 渲染 worker bubble + 角色色 who", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 50));
    api.pushTask(TASK_ID, ev({ type: "agent_responded", text: "查到 12 条" }, { agent: LUBAN }));
    await new Promise((r) => setTimeout(r, 30));
    const wb = queryAllByTestId("msg-worker");
    expect(wb).toHaveLength(1);
    expect(wb[0]?.textContent).toContain("查到 12 条");
    expect(wb[0]?.textContent).toContain("鲁班");
    unmount();
  });

  // #4（2026-06-04 用户选「按门客分 sub-tab」）：一个任务多门客并行时，顶部出
  // worker sub-tab 栏，点选过滤到该门客的消息流；只有 1 个门客时不出 sub-tab。
  it("#4 · 多门客 → 出 worker sub-tab 栏 + 点选过滤到该门客流", async () => {
    const { api, getByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 50));
    api.pushTask(TASK_ID, ev({ type: "agent_responded", text: "鲁班查到 12 条" }, { agent: LUBAN }));
    api.pushTask(TASK_ID, ev({ type: "agent_responded", text: "蒲松写完文档" }, { agent: PUSONG }));
    await new Promise((r) => setTimeout(r, 30));

    // sub-tab 栏出现（≥2 门客），含「全部」+ 两个门客名
    const subtabs = getByTestId("worker-subtabs");
    expect(subtabs.textContent).toContain("全部");
    expect(subtabs.textContent).toContain("鲁班");
    expect(subtabs.textContent).toContain("蒲松");

    // 默认「全部」：两条都在
    expect(getByTestId("task-thread").textContent).toContain("鲁班查到 12 条");
    expect(getByTestId("task-thread").textContent).toContain("蒲松写完文档");

    // 点「蒲松」→ 仅蒲松的流（鲁班的隐藏）
    fireEvent.click(getByTestId(`worker-subtab-${PUSONG}`));
    await new Promise((r) => setTimeout(r, 10));
    expect(getByTestId("task-thread").textContent).toContain("蒲松写完文档");
    expect(getByTestId("task-thread").textContent).not.toContain("鲁班查到 12 条");
    unmount();
  });

  it("#4 · 仅 1 个门客 · 不出 sub-tab 栏（无意义不打扰）", async () => {
    const oneWorker: TasksOverview = {
      running: [
        {
          ...TASK_OVERVIEW.running[0]!,
          members: [
            { agent_id: XUANNV, role: "xuannv", role_display: "玄女", status: "busy" as const },
            { agent_id: LUBAN, role: "luban", role_display: "鲁班", status: "busy" as const },
          ],
        },
      ],
      completed: [],
    };
    const { queryByTestId, unmount } = setup({ overview: oneWorker });
    await new Promise((r) => setTimeout(r, 50));
    expect(queryByTestId("worker-subtabs")).toBeNull();
    unmount();
  });

  it("发送 → user bubble + intervene 带 task_id", async () => {
    const { api, getByTestId, queryAllByTestId, unmount } = setup({ interveneSeq: [200] });
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "再查一下" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    const req = api.state.intervenes[0] as { task_id?: string; text: string };
    expect(req.task_id).toBe(TASK_ID);
    expect(req.text).toBe("再查一下");
    expect(queryAllByTestId("msg-user")).toHaveLength(1);
    unmount();
  });

  it("@ chip · intervene 带 target+mentions+task_id", async () => {
    const { api, getByTestId, unmount } = setup({ interveneSeq: [200] });
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    fireEvent.click(getByTestId("mention-item-" + LUBAN));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 帮我看" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    const req = api.state.intervenes[0] as {
      task_id?: string;
      target?: string;
      mentions?: string[];
    };
    expect(req.task_id).toBe(TASK_ID);
    expect(req.target).toBe(LUBAN);
    expect(req.mentions).toEqual([LUBAN]);
    unmount();
  });

  it("v3 #52 · 任务 status=completed 时不下发 task_id（避免误中 4xx）", async () => {
    const overview = {
      running: [],
      completed: [
        {
          id: TASK_ID,
          title: "查 ERP API",
          status: "completed" as const,
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 12_000,
          members: [
            { agent_id: XUANNV, role: "xuannv", role_display: "玄女", status: "idle" as const },
          ],
        },
      ],
    };
    const { api, getByTestId, unmount } = setup({ overview, interveneSeq: [200] });
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "你好" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    const req = api.state.intervenes[0] as { task_id?: string | null };
    // 关键：task_id 不应是 TASK_ID（task 已完成，避免 backend 把 intervene 路由到 closed task）
    expect(req.task_id).toBeNull();
    unmount();
  });

  it("4xx · 无 @ chip · toast \"玄女正忙\"（无 worker target = 玄女路径）", async () => {
    const { getByTestId, queryByTestId, unmount } = setup({ interveneSeq: [409] });
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "插一句" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("toast")?.textContent).toContain("玄女正忙");
    unmount();
  });

  it("v3 #68 (b) · 4xx · @worker chip · toast \"门客正忙\"（task running + worker target）", async () => {
    const { getByTestId, queryByTestId, unmount } = setup({ interveneSeq: [409] });
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    fireEvent.click(getByTestId("mention-item-" + LUBAN));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 帮我看" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("toast")?.textContent).toContain("门客正忙");
    unmount();
  });

  it("v3 #68 (b) · 任务 completed + 用户发文 4xx · toast \"玄女正忙\" 不是 \"门客正忙\"（用户撞 bug）", async () => {
    // 模拟用户场景：任务 done 后在 thread composer 输 "hi" → 玄女自己 busy → 4xx
    // 期望：toast 文案是"玄女正忙"，不是误导的"门客正忙"
    const overview: TasksOverview = {
      running: [],
      completed: [
        {
          id: TASK_ID,
          title: "查 ERP API",
          status: "completed" as const,
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 19_000,
          members: [
            { agent_id: LUBAN, role: "luban", role_display: "鲁班", status: "idle" as const, node_id: "home" },
          ],
        },
      ],
    };
    const { getByTestId, queryByTestId, unmount } = setup({ overview, interveneSeq: [409] });
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryByTestId("toast")?.textContent).toContain("玄女正忙");
    expect(queryByTestId("toast")?.textContent).not.toContain("门客正忙");
    unmount();
  });

  it("P0.E · 任务 completed · composer placeholder 改成「跟玄女继续聊」（默认是「对玄女说」）", async () => {
    // 默认场景（task running）— placeholder 是默认文案
    const { getByTestId: g1, unmount: u1 } = setup();
    await new Promise((r) => setTimeout(r, 50));
    const editor1 = g1("mention-editor") as HTMLTextAreaElement;
    expect(editor1.placeholder).toMatch(/对玄女说/);
    u1();

    const overview: TasksOverview = {
      running: [],
      completed: [
        {
          id: TASK_ID,
          title: "查 ERP API",
          status: "completed" as const,
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 19_000,
          members: [
            { agent_id: XUANNV, role: "xuannv", role_display: "玄女", status: "idle" as const },
          ],
        },
      ],
    };
    const { getByTestId: g2, unmount: u2 } = setup({ overview });
    await new Promise((r) => setTimeout(r, 50));
    const editor2 = g2("mention-editor") as HTMLTextAreaElement;
    expect(editor2.placeholder).toMatch(/任务已完结/);
    expect(editor2.placeholder).toMatch(/玄女/);
    u2();
  });

  it("v3 #N · 已歇消歧 · 任务 completed + worker idle · banner 显 \"已歇 · 待命\"（仍可召回）", async () => {
    // 旧版 fallback 裸 "已歇" 让用户分不清 worker 死活——现在加副标识。
    const overview: TasksOverview = {
      running: [],
      completed: [
        {
          id: TASK_ID,
          title: "查 ERP API",
          status: "completed" as const,
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 19_000,
          members: [
            { agent_id: LUBAN, role: "luban", role_display: "鲁班", status: "idle" as const, node_id: "home" },
          ],
        },
      ],
    };
    const { findByTestId, unmount } = setup({ overview });
    const members = await findByTestId("task-banner-members");
    expect(members.textContent).toContain("已歇 · 待命");
    unmount();
  });

  // ---- v3 #N · statusLabel/memberTail 矩阵覆盖 ----------------------
  // matrix · 行 = (task.status, member.status)，列 = (banner memberTail, popover statusLabel)
  // running 行 banner 用 fallback 路径（没 last_activity / tool_call / activity） · popover 总走 statusLabel
  describe("v3 #N · 已歇消歧矩阵", () => {
    type Row = {
      taskStatus: "running" | "completed" | "failed";
      memberStatus: "idle" | "thinking" | "dead" | "busy";
      label: string;
    };
    const rows: Row[] = [
      { taskStatus: "running", memberStatus: "idle", label: "待命" },
      { taskStatus: "running", memberStatus: "thinking", label: "思考中" },
      { taskStatus: "running", memberStatus: "dead", label: "已下线" },
      { taskStatus: "running", memberStatus: "busy", label: "运行中" },
      { taskStatus: "completed", memberStatus: "idle", label: "已歇 · 待命" },
      { taskStatus: "completed", memberStatus: "thinking", label: "已歇 · 收尾中" },
      { taskStatus: "completed", memberStatus: "dead", label: "已歇 · 已下线" },
      { taskStatus: "completed", memberStatus: "busy", label: "已歇" },
    ];
    for (const row of rows) {
      it(`task=${row.taskStatus} · worker=${row.memberStatus} → "${row.label}"`, async () => {
        const member = {
          agent_id: LUBAN,
          role: "luban",
          role_display: "鲁班",
          status: row.memberStatus,
          node_id: "home",
          // 故意不填 last_activity / last_tool_call / activity → 走 fallback 路径
        };
        const card = {
          id: TASK_ID,
          title: "矩阵测试",
          status: row.taskStatus,
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T11:12:00Z",
          duration_ms: 19_000,
          members: [member],
        };
        const overview: TasksOverview = {
          running: row.taskStatus === "running" ? [card] : [],
          completed: row.taskStatus !== "running" ? [card] : [],
        } as TasksOverview;
        const { findByTestId, getByTestId, unmount } = setup({ overview });
        // banner memberTail
        const banner = await findByTestId("task-banner-members");
        expect(banner.textContent).toContain(row.label);
        // popover statusLabel · 点击 ⋯ 打开
        fireEvent.click(getByTestId("task-thread-more"));
        const menuItem = await findByTestId(`menu-member-${LUBAN}`);
        expect(menuItem.textContent).toContain(row.label);
        unmount();
      });
    }
  });

  it("history fold · pre-load events → thread 已含历史", async () => {
    const events: ServerEvent[] = [
      ev(
        {
          type: "user_intervention_sent",
          target: XUANNV,
          text: "查 ERP",
          mentions: [],
        } as ServerEvent["kind"],
        { agent: XUANNV, id: "h-1" },
      ),
      ev({ type: "agent_responded", text: "好的" }, { agent: XUANNV, id: "h-2" }),
      ev({ type: "agent_responded", text: "查到了" }, { agent: LUBAN, id: "h-3" }),
    ];
    const { queryAllByTestId, unmount } = setup({ taskEvents: events });
    await new Promise((r) => setTimeout(r, 80));
    expect(queryAllByTestId("msg-user")).toHaveLength(1);
    expect(queryAllByTestId("msg-xuannv")).toHaveLength(1);
    expect(queryAllByTestId("msg-worker")).toHaveLength(1);
    unmount();
  });

  it("composer 候选 = 任务全成员（含玄女）", async () => {
    const { getByTestId, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    const items = queryAllByTestId(/^mention-item-/);
    expect(items.length).toBe(3);
    // 玄女也是候选
    expect(items.map((e) => e.textContent).join(" ")).toContain("玄女");
    expect(items.map((e) => e.textContent).join(" ")).toContain("鲁班");
    unmount();
  });

  it("v3 #60 · composer 候选含 dist online 节点（worker 在前 节点在后）", async () => {
    const api = createMockApi({
      tasksOverview: TASK_OVERVIEW,
      nodes: {
        nodes: [
          {
            node_id: "home",
            tags: ["home"],
            max_concurrency: 4,
            inflight_jobs: 1,
            heartbeat_lag_ms: 100,
            online: true,
            registered_at: null,
            workers: [],
          },
          {
            node_id: "mac-local",
            tags: ["local"],
            max_concurrency: 8,
            inflight_jobs: 0,
            heartbeat_lag_ms: 800,
            online: true,
            registered_at: null,
            workers: [],
          },
          {
            node_id: "off",
            tags: [],
            max_concurrency: 1,
            inflight_jobs: 0,
            heartbeat_lag_ms: 99999,
            online: false,
            registered_at: null,
            workers: [],
          },
        ],
      },
    });
    setApiOverride(api);
    const { getByTestId, queryByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3}>
        <TaskThreadPage task_id={TASK_ID} fallback_title="查 ERP API" />
      </ApiProvider>
    ));
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    // 3 worker + 2 online node = 5；节点段 separator 应在
    const items = queryAllByTestId(/^mention-item-/);
    expect(items.length).toBe(5);
    expect(getByTestId("mention-popup-node-sep")).toBeTruthy();
    expect(getByTestId("mention-item-node-home")).toBeTruthy();
    expect(getByTestId("mention-item-node-mac-local")).toBeTruthy();
    // offline 节点不入候选
    expect(queryByTestId("mention-item-node-off")).toBeNull();
    unmount();
  });

  it("v3 #60 · @ node 发送 · intervene body 含 pinned_node + task_id", async () => {
    const api = createMockApi({
      tasksOverview: TASK_OVERVIEW,
      nodes: {
        nodes: [
          {
            node_id: "mac-local",
            tags: ["local"],
            max_concurrency: 8,
            inflight_jobs: 0,
            heartbeat_lag_ms: 800,
            online: true,
            registered_at: null,
            workers: [],
          },
        ],
      },
      interveneSeq: [200],
    });
    setApiOverride(api);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3}>
        <TaskThreadPage task_id={TASK_ID} fallback_title="查 ERP API" />
      </ApiProvider>
    ));
    await new Promise((r) => setTimeout(r, 50));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "用 @ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 跑测试" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    const req = api.state.intervenes[0] as {
      task_id?: string | null;
      pinned_node?: string;
      target?: string;
    };
    expect(req.pinned_node).toBe("mac-local");
    expect(req.task_id).toBe(TASK_ID);
    // 仅 node chip · target 应缺省（让 backend 走玄女默认）
    expect(req.target).toBeUndefined();
    unmount();
  });
});
