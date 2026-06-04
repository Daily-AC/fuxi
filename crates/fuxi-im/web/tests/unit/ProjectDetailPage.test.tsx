import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { ProjectDetailPage } from "~/views/pages/ProjectDetailPage";
import { createMockApi } from "../mocks/api";
import type {
  EphemeralResponse,
  ProjectsResponse,
  SandboxView,
} from "~/types/api";

afterEach(() => setApiOverride(null));

function setup(args: {
  projects?: ProjectsResponse;
  sandboxesByProject?: Record<string, SandboxView[]>;
  ephemeralByProject?: Record<string, EphemeralResponse>;
} = {}) {
  setApiOverride(createMockApi(args));
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={4} initialMoreSub="projects">
      <ProjectDetailPage project_id="erp" />
    </ApiProvider>
  ));
}

describe("ProjectDetailPage", () => {
  it("加载中态：projects 未到 → 显 loading 不崩", async () => {
    const { findByTestId } = setup();
    await findByTestId("project-detail-loading");
  });

  it("project 命中 → 渲染 path + branch + 三段标题（sandbox / L2 / 交付）", async () => {
    const { findByTestId, findByText } = setup({
      projects: {
        projects: [
          {
            id: "erp",
            canonical_path: "/Users/e0_7/erp",
            default_branch: "main",
            created_at: "2026-05-01T00:00:00Z",
          },
        ],
      },
      sandboxesByProject: {
        erp: [
          {
            role: "luban",
            workspace_id: "erp/L3/luban",
            path: "/Users/e0_7/.fuxi/projects/erp/sandboxes/luban",
            branch: "luban/erp-main",
          },
        ],
      },
    });
    const meta = await findByTestId("project-detail-meta");
    expect(meta.textContent).toContain("/Users/e0_7/erp");
    expect(meta.textContent).toContain("main");
    await findByText("L3 持久 sandbox");
    await findByText("L2 一次性工作区");
    await findByText("交付");
    await findByTestId("pdetail-sandbox-erp/L3/luban");
  });

  it("L2 active + archived 都展示", async () => {
    const TASK_A = "task-aaaaaaaa-1111-2222-3333-444444444444";
    const TASK_B = "task-bbbbbbbb-1111-2222-3333-444444444444";
    const { findByTestId } = setup({
      projects: {
        projects: [
          {
            id: "erp",
            canonical_path: "/Users/e0_7/erp",
            default_branch: "main",
            created_at: "2026-05-01T00:00:00Z",
          },
        ],
      },
      ephemeralByProject: {
        erp: {
          active: [
            {
              task: TASK_A,
              workspace_id: `erp/L2/${TASK_A.slice(5)}`,
              branch: `task/${TASK_A.slice(5)}`,
              path: `/Users/e0_7/.fuxi/projects/erp/ephemeral/${TASK_A}`,
            },
          ],
          archived: [
            {
              task: TASK_B,
              workspace_id: `erp/L2/${TASK_B.slice(5)}`,
              branch: `task/${TASK_B.slice(5)}`,
              archived_at: "2026-05-02T01:00:00Z",
              archive_path: `/Users/e0_7/.fuxi/projects/erp/archive/${TASK_B}`,
            },
          ],
        },
      },
    });
    await findByTestId(`pdetail-l2-active-erp/L2/${TASK_A.slice(5)}`);
    await findByTestId(`pdetail-l2-archived-erp/L2/${TASK_B.slice(5)}`);
  });

  it("无 L3 sandbox → 显空态 hint", async () => {
    const { findByText } = setup({
      projects: {
        projects: [
          {
            id: "erp",
            canonical_path: "/x",
            default_branch: "main",
            created_at: "2026-05-01T00:00:00Z",
          },
        ],
      },
    });
    await findByText("L3 持久 sandbox");
    // 空态 hint 含 spawn 命令片段
    const hint = await findByText(
      (content) => content.includes("fuxi spawn --role"),
    );
    expect(hint).toBeTruthy();
  });
});
