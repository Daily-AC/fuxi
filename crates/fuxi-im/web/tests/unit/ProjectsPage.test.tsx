import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { ProjectsPage } from "~/views/pages/ProjectsPage";
import { createMockApi } from "../mocks/api";
import type { ProjectsResponse, SandboxView } from "~/types/api";

afterEach(() => setApiOverride(null));

function setup(
  projects?: ProjectsResponse,
  sandboxesByProject?: Record<string, SandboxView[]>,
) {
  const api = createMockApi({ projects, sandboxesByProject });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={4} initialMoreSub="projects">
      <ProjectsPage />
    </ApiProvider>
  ));
}

describe("ProjectsPage", () => {
  it("renders empty state with hint to use CLI", async () => {
    const { findByTestId } = setup({ projects: [] });
    const empty = await findByTestId("projects-empty");
    expect(empty.textContent).toContain("暂无项目");
    expect(empty.textContent).toContain("fuxi project add");
  });

  it("renders one project card per registered project", async () => {
    const { findByTestId } = setup({
      projects: [
        {
          id: "erp",
          canonical_path: "/Users/e0_7/erp",
          default_branch: "main",
          created_at: "2026-05-02T10:00:00Z",
        },
        {
          id: "fuxi",
          canonical_path: "/Users/e0_7/fuxi",
          default_branch: "main",
          created_at: "2026-04-01T10:00:00Z",
        },
      ],
    });
    const erp = await findByTestId("project-card-erp");
    expect(erp.textContent).toContain("erp");
    expect(erp.textContent).toContain("/Users/e0_7/erp");
    expect(erp.textContent).toContain("main");

    const fuxi = await findByTestId("project-card-fuxi");
    expect(fuxi.textContent).toContain("fuxi");
    expect(fuxi.textContent).toContain("/Users/e0_7/fuxi");
  });

  it("shows loading then renders", async () => {
    // setup 调完 mock 已 ready，不容易测真 loading；至少验渲染不挂
    const { findByTestId } = setup({ projects: [] });
    expect(await findByTestId("page-projects")).toBeTruthy();
  });

  it("clicking + 注册 opens modal", async () => {
    const { findByTestId, queryByTestId } = setup({ projects: [] });
    expect(queryByTestId("projects-add-modal")).toBeNull();
    const btn = await findByTestId("projects-add-btn");
    btn.click();
    const modal = await findByTestId("projects-add-modal");
    expect(modal).toBeTruthy();
  });

  it("submitting modal with new path adds project to list", async () => {
    const { findByTestId, queryByTestId } = setup({ projects: [] });

    // 打开 modal
    (await findByTestId("projects-add-btn")).click();

    // 填表 + 提交
    const path = (await findByTestId("projects-add-path")) as HTMLInputElement;
    path.value = "/Users/e0_7/erp";
    path.dispatchEvent(new Event("input", { bubbles: true }));
    const name = (await findByTestId("projects-add-name")) as HTMLInputElement;
    name.value = "erp";
    name.dispatchEvent(new Event("input", { bubbles: true }));

    const submit = await findByTestId("projects-add-submit");
    submit.click();

    // mock addProject sync resolve→ refetch → list 出现 erp 卡
    // solid-js 异步 effect 需要 microtask flush
    await new Promise((r) => setTimeout(r, 10));
    const card = await findByTestId("project-card-erp");
    expect(card).toBeTruthy();
    expect(queryByTestId("projects-add-modal")).toBeNull(); // modal 关掉
  });

  it("renders sandbox list when project has L3 sandboxes", async () => {
    const { findByTestId } = setup(
      {
        projects: [
          {
            id: "erp",
            canonical_path: "/Users/e0_7/erp",
            default_branch: "main",
            created_at: "2026-05-02T10:00:00Z",
          },
        ],
      },
      {
        erp: [
          {
            role: "luban",
            workspace_id: "erp/L3/luban",
            path: "/Users/e0_7/.fuxi/projects/erp/sandboxes/luban",
            branch: "luban/erp-main",
          },
        ],
      },
    );
    const list = await findByTestId("sandboxes-erp");
    expect(list.textContent).toContain("luban");
    expect(list.textContent).toContain("luban/erp-main");
  });

  it("renders empty hint when project has no sandbox", async () => {
    const { findByTestId } = setup(
      {
        projects: [
          {
            id: "erp",
            canonical_path: "/Users/e0_7/erp",
            default_branch: "main",
            created_at: "2026-05-02T10:00:00Z",
          },
        ],
      },
      { erp: [] },
    );
    const empty = await findByTestId("sandboxes-empty-erp");
    expect(empty.textContent).toContain("暂无 sandbox");
    expect(empty.textContent).toContain("fuxi spawn");
  });

  it("delete confirmation: cancel keeps card; confirm removes", async () => {
    const { findByTestId, queryByTestId } = setup({
      projects: [
        {
          id: "doomed",
          canonical_path: "/tmp/x",
          default_branch: "main",
          created_at: "2026-05-02T10:00:00Z",
        },
      ],
    });

    // 点 删除 → 进 confirming
    (await findByTestId("project-delete-doomed")).click();
    expect(queryByTestId("project-delete-confirm-doomed")).toBeTruthy();

    // 取消 → confirming 收掉
    (await findByTestId("project-delete-cancel-doomed")).click();
    await new Promise((r) => setTimeout(r, 5));
    expect(queryByTestId("project-delete-confirm-doomed")).toBeNull();
    // 卡仍在
    expect(queryByTestId("project-card-doomed")).toBeTruthy();

    // 再点 删除 → 确认 → 卡消失
    (await findByTestId("project-delete-doomed")).click();
    (await findByTestId("project-delete-confirm-doomed")).click();
    await new Promise((r) => setTimeout(r, 10));
    expect(queryByTestId("project-card-doomed")).toBeNull();
  });
});
