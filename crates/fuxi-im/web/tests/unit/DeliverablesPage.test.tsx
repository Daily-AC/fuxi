import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { DeliverablesPage } from "~/views/pages/DeliverablesPage";
import { createMockApi } from "../mocks/api";
import type { DeliverablesResponse } from "~/types/api";

afterEach(() => setApiOverride(null));

function setup(deliverables?: DeliverablesResponse) {
  const api = createMockApi({ deliverables });
  setApiOverride(api);
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={3}>
      <DeliverablesPage />
    </ApiProvider>
  ));
}

const TASK_UUID = "5e5e98b4-1cdf-44d5-8bb3-d489c7905392";

describe("DeliverablesPage", () => {
  it("renders empty state when no deliverables", async () => {
    const { findByTestId } = setup({ deliverables: [] });
    const empty = await findByTestId("deliverables-empty");
    expect(empty.textContent).toContain("暂无交付");
  });

  it("renders entry card with kind label + project + files", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "research_summary",
          files: [
            { name: "report.md", sha256: "abc123def4567890", size_bytes: 4096 },
            { name: "data.csv", sha256: "fedcba0987654321", size_bytes: 1024 },
          ],
          produced_at: "2026-05-02T10:00:00Z",
        },
      ],
    });
    const card = await findByTestId(
      `deliverable-card-${TASK_UUID}-research_summary`,
    );
    // kind 中文标签
    expect(card.textContent).toContain("调研");
    // 项目 + task short
    expect(card.textContent).toContain("erp");
    expect(card.textContent).toContain("task-5e5e98b4");
    // 文件名 + 大小
    expect(card.textContent).toContain("report.md");
    expect(card.textContent).toContain("data.csv");
    expect(card.textContent).toContain("4.0KB");
    expect(card.textContent).toContain("1.0KB");
  });

  it("file link points to deliverableFileUrl with task- prefix", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "code_change",
          files: [{ name: "patch.diff", sha256: "1234", size_bytes: 100 }],
          produced_at: "2026-05-02T10:00:00Z",
        },
      ],
    });
    const link = (await findByTestId(
      `deliverable-file-${TASK_UUID}-patch.diff`,
    )) as HTMLAnchorElement;
    expect(link.href).toContain(`/api/deliverables/erp/task-${TASK_UUID}/files/patch.diff`);
    expect(link.getAttribute("download")).toBe("patch.diff");
  });

  it("kind label maps each of 5 kinds", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "error_block",
          files: [{ name: "err.txt", sha256: "x", size_bytes: 10 }],
          produced_at: "2026-05-02T10:00:00Z",
        },
      ],
    });
    const card = await findByTestId(
      `deliverable-card-${TASK_UUID}-error_block`,
    );
    expect(card.textContent).toContain("错误阻塞");
  });
});
