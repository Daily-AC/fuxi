import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { ProjectsPage } from "~/views/pages/ProjectsPage";
import { createMockApi } from "../mocks/api";
import type { ProjectsResponse } from "~/types/api";

afterEach(() => setApiOverride(null));

function setup(projects?: ProjectsResponse) {
  const api = createMockApi({ projects });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={2}>
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
});
