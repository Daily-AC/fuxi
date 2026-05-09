import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { MemoryPage } from "~/views/pages/MemoryPage";
import { createMockApi } from "../mocks/api";

afterEach(() => setApiOverride(null));

describe("MemoryPage", () => {
  it("空状态 · 显引导文案（v1-session19 起 3 层全空才提示）", async () => {
    setApiOverride(
      createMockApi({
        memory: { groups: [], total: 0, insights: [], user_profile: [] },
      }),
    );
    const { findByText, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <MemoryPage />
      </ApiProvider>
    ));
    await findByText(/记忆目前空着/);
    unmount();
  });

  it("事实策府 section · 按 subject 分组渲染 + 计数", async () => {
    setApiOverride(
      createMockApi({
        memory: {
          total: 3,
          insights: [],
          user_profile: [],
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
    expect(getByTestId("memory-section-facts").textContent).toContain("共 3 条");
    expect(getByTestId("memory-fact-f1").textContent).toContain("冰美式");
    expect(getByTestId("memory-fact-f3").textContent).toContain("工匠");
    expect(getByTestId("memory-group-luban")).toBeTruthy();
    unmount();
  });

  it("v1-session19 · insights + user_profile 三层并存", async () => {
    setApiOverride(
      createMockApi({
        memory: {
          total: 1,
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
              ],
            },
          ],
          insights: [
            {
              id: "h1",
              role: "luban",
              task_type: "refactor",
              pattern: "拆函数前先列调用方再 grep 重命名",
              confidence: 0.85,
              created_at: "2026-05-07T00:00:00Z",
            },
          ],
          user_profile: [
            {
              id: "p1",
              key: "identity",
              value: "用户名 = 以琳",
              source: "manual",
              updated_at: "2026-05-07T00:00:00Z",
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
    // 三个 section 都应渲染
    await findByTestId("memory-section-profile");
    expect(getByTestId("memory-section-facts")).toBeTruthy();
    expect(getByTestId("memory-section-insights")).toBeTruthy();
    // 各 section 内容
    expect(getByTestId("memory-profile-p1").textContent).toContain("以琳");
    expect(getByTestId("memory-fact-f1").textContent).toContain("冰美式");
    expect(getByTestId("memory-insight-h1").textContent).toContain("调用方");
    unmount();
  });

  it("仅有 insights 时 · 不显事实策府 section", async () => {
    setApiOverride(
      createMockApi({
        memory: {
          total: 0,
          groups: [],
          user_profile: [],
          insights: [
            {
              id: "h1",
              role: "cangjie",
              task_type: "validation",
              pattern: "中间环节加 checkpoint 观察",
              confidence: 0.7,
              created_at: "2026-05-07T00:00:00Z",
            },
          ],
        },
      }),
    );
    const { findByTestId, queryByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <MemoryPage />
      </ApiProvider>
    ));
    await findByTestId("memory-section-insights");
    expect(queryByTestId("memory-section-facts")).toBeNull();
    expect(queryByTestId("memory-section-profile")).toBeNull();
    unmount();
  });
});
