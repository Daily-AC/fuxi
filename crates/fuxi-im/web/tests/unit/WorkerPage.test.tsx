import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { WorkerPage } from "~/views/pages/WorkerPage";
import { Toast } from "~/components/Toast";
import { dismissToast } from "~/lib/toast";
import { createMockApi } from "../mocks/api";
import type { ServerEvent } from "~/types/events";
import type { TasksOverview } from "~/types/api";

const LUBAN = "agent-luban-uuid";

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
      agent: meta.agent === undefined ? LUBAN : meta.agent,
      task: meta.task ?? null,
    },
    kind,
  };
}

function setup(opts?: {
  interveneSeq?: number[];
  workerEvents?: ServerEvent[];
  tasksOverview?: TasksOverview;
}) {
  const api = createMockApi({
    interveneSeq: opts?.interveneSeq,
    events: opts?.workerEvents ? { [`worker:${LUBAN}`]: opts.workerEvents } : {},
    tasksOverview: opts?.tasksOverview,
  });
  setApiOverride(api);
  const tools = render(() => (
    <ApiProvider initialAuth="in">
      <WorkerPage agent_id={LUBAN} role_display="鲁班" />
      <Toast />
    </ApiProvider>
  ));
  return { api, ...tools };
}

describe("WorkerPage · 私聊页 modal (#N3)", () => {
  it("渲染顶栏 · ‹ 玄女 + 角色名 + agent 短 id", async () => {
    const { getByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    const back = getByTestId("worker-back");
    expect(back.textContent).toContain("玄女");
    const page = getByTestId("page-worker");
    expect(page.getAttribute("data-agent-id")).toBe(LUBAN);
    expect(page.textContent).toContain("鲁班");
    unmount();
  });

  it("WS agent_responded · 渲染 worker bubble (橙 who)", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushWorker(LUBAN, ev({ type: "agent_responded", text: "查到了 ERP 的 12 条接口" }));
    await new Promise((r) => setTimeout(r, 30));
    const wb = queryAllByTestId("msg-worker");
    expect(wb).toHaveLength(1);
    expect(wb[0]?.textContent).toContain("查到了 ERP 的 12 条接口");
    expect(wb[0]?.textContent).toContain("鲁班");
    unmount();
  });

  it("非该 agent 的 WS 事件 · 不渲染（filter 防御）", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushWorker(
      LUBAN,
      ev({ type: "agent_responded", text: "墨子说话" }, { agent: "agent-mozi-uuid" }),
    );
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId("msg-worker")).toHaveLength(0);
    unmount();
  });

  it("发送 → 用户气泡上屏 + intervene 带 target=agent_id", async () => {
    const { api, getByTestId, queryAllByTestId, unmount } = setup({ interveneSeq: [200] });
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByTestId("composer-input"), { target: { value: "查 ERP API" } });
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    const req = api.state.intervenes[0] as { target?: string; text: string };
    expect(req.target).toBe(LUBAN);
    expect(req.text).toBe("查 ERP API");
    expect(queryAllByTestId("msg-user")).toHaveLength(1);
    unmount();
  });

  it("intervene 4xx → toast 显 \"门客正忙\"", async () => {
    const { getByTestId, queryByTestId, unmount } = setup({ interveneSeq: [409] });
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByTestId("composer-input"), { target: { value: "插一句" } });
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 30));
    const toast = queryByTestId("toast");
    expect(toast).toBeTruthy();
    expect(toast?.textContent).toContain("门客正忙");
    unmount();
  });

  it("history fold · pre-load events → thread 已含历史", async () => {
    const events: ServerEvent[] = [
      ev(
        { type: "user_intervention_sent", target: LUBAN, text: "查 ERP" } as ServerEvent["kind"],
        { agent: "agent-xuannv-uuid", id: "h-1" },
      ),
      ev({ type: "agent_responded", text: "找到了" }, { id: "h-2" }),
    ];
    const { queryAllByTestId, unmount } = setup({ workerEvents: events });
    await new Promise((r) => setTimeout(r, 50));
    expect(queryAllByTestId("msg-user")).toHaveLength(1);
    expect(queryAllByTestId("msg-worker")).toHaveLength(1);
    unmount();
  });

  it("tool_call_started + tool_call_finished 配对 · ToolCallCard 折叠卡显完成", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushWorker(
      LUBAN,
      ev(
        { type: "tool_call_started", tool: "Bash", args: "grep server" },
        { at: "2026-04-26T12:00:00Z", id: "t1" },
      ),
    );
    api.pushWorker(
      LUBAN,
      ev(
        { type: "tool_call_finished", tool: "Bash", ok: true, output_preview: "main.go:1" },
        { at: "2026-04-26T12:00:00.5Z", id: "t2" },
      ),
    );
    await new Promise((r) => setTimeout(r, 30));
    const cards = queryAllByTestId(/^tool-card-/);
    expect(cards).toHaveLength(1);
    expect(cards[0]?.getAttribute("data-status")).toBe("ok");
    unmount();
  });

  it("agent_idle → 居中 marker 出现", async () => {
    const { api, queryAllByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    api.pushWorker(LUBAN, ev({ type: "agent_idle" }, { id: "mk-1" }));
    await new Promise((r) => setTimeout(r, 30));
    const markers = queryAllByTestId(/^marker-/);
    expect(markers).toHaveLength(1);
    expect(markers[0]?.textContent).toContain("idle");
    unmount();
  });

  it("composer placeholder 是 \"跟鲁班说……\"", async () => {
    const { getByTestId, unmount } = setup();
    await new Promise((r) => setTimeout(r, 30));
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    expect(input.placeholder).toContain("鲁班");
    unmount();
  });

  it("任务 banner · /api/tasks 找到该 agent 的 task → 显标题 + duration", async () => {
    const overview: TasksOverview = {
      running: [
        {
          id: "task-uuid-erp",
          title: "查 ERP API",
          status: "running",
          created_at: "2026-04-26T11:00:00Z",
          last_active_at: "2026-04-26T12:00:00Z",
          duration_ms: 12_000,
          members: [
            {
              agent_id: LUBAN,
              role: "luban",
              role_display: "鲁班",
              status: "busy",
            },
          ],
        },
      ],
      completed: [],
    };
    const { getByTestId, unmount } = setup({ tasksOverview: overview });
    await new Promise((r) => setTimeout(r, 50));
    const banner = getByTestId("worker-task-banner");
    expect(banner.textContent).toContain("查 ERP API");
    expect(banner.textContent).toContain("0:12");
    expect(banner.textContent).toContain("进行中");
    unmount();
  });
});
