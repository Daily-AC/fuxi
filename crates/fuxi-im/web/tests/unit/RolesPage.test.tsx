import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { RolesPage } from "~/views/pages/RolesPage";
import { createMockApi } from "../mocks/api";

afterEach(() => setApiOverride(null));

describe("RolesPage", () => {
  it("空状态文案", async () => {
    setApiOverride(createMockApi({ roles: { roles: [] } }));
    const { findByText, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <RolesPage />
      </ApiProvider>
    ));
    await findByText(/未发现 role/);
    unmount();
  });

  it("非空 · 渲染 role 卡含 name / desc / tier / chips", async () => {
    setApiOverride(
      createMockApi({
        roles: {
          roles: [
            {
              id: "luban",
              name: "鲁班",
              description: "工匠门客",
              tier: "worker",
              cli: "claude-code",
              allowed_tools: "Read Write",
              has_instructions: true,
              has_examples: true,
              has_resources: false,
            },
          ],
        },
      }),
    );
    const { findByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <RolesPage />
      </ApiProvider>
    ));
    const card = await findByTestId("role-card-luban");
    expect(card.textContent).toContain("鲁班");
    expect(card.textContent).toContain("工匠门客");
    expect(card.textContent?.toLowerCase()).toContain("worker");
    expect(card.textContent).toContain("claude-code");
    expect(card.textContent).toContain("instructions");
    expect(card.textContent).toContain("examples");
    expect(card.textContent).not.toContain("resources");
    unmount();
  });
});
