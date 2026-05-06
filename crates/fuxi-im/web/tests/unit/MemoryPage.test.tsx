import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { MemoryPage } from "~/views/pages/MemoryPage";
import { createMockApi } from "../mocks/api";

afterEach(() => setApiOverride(null));

describe("MemoryPage", () => {
  it("空状态 · 显引导文案", async () => {
    setApiOverride(createMockApi({ memory: { groups: [], total: 0 } }));
    const { findByText, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <MemoryPage />
      </ApiProvider>
    ));
    await findByText(/策府目前空着/);
    unmount();
  });

  it("非空 · 按 subject 分组渲染 + 总数 summary", async () => {
    setApiOverride(
      createMockApi({
        memory: {
          total: 3,
          groups: [
            {
              subject: "user",
              facts: [
                {
                  id: "f1",
                  subject: "user",
                  predicate: "prefers",
                  object: "冰美式",
                  source: "manual",
                  confidence: 0.9,
                  updated_at: "2026-05-07T00:00:00Z",
                },
                {
                  id: "f2",
                  subject: "user",
                  predicate: "name",
                  object: "以琳",
                  source: "manual",
                  confidence: 0.95,
                  updated_at: "2026-05-07T00:00:01Z",
                },
              ],
            },
            {
              subject: "luban",
              facts: [
                {
                  id: "f3",
                  subject: "luban",
                  predicate: "role",
                  object: "工匠",
                  source: "agent",
                  confidence: 0.7,
                  updated_at: "2026-05-07T00:00:02Z",
                },
              ],
            },
          ],
        },
      }),
    );
    const { findByTestId, getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <MemoryPage />
      </ApiProvider>
    ));
    await findByTestId("memory-group-user");
    expect(getByTestId("memory-summary").textContent).toContain("3 条");
    expect(getByTestId("memory-fact-f1").textContent).toContain("冰美式");
    expect(getByTestId("memory-fact-f3").textContent).toContain("工匠");
    expect(getByTestId("memory-group-luban")).toBeTruthy();
    unmount();
  });
});
