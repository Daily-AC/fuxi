import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { DeliverableDetailPage } from "~/views/pages/DeliverableDetailPage";
import { createMockApi } from "../mocks/api";
import type { DeliverablesResponse } from "~/types/api";

afterEach(() => setApiOverride(null));

const TASK_UUID = "5e5e98b4-1cdf-44d5-8bb3-d489c7905392";

function setup(deliverables?: DeliverablesResponse) {
  setApiOverride(createMockApi({ deliverables }));
  return render(() => (
    <ApiProvider initialAuth="in" initialTab={3}>
      <DeliverableDetailPage project_id="erp" task_id={TASK_UUID} />
    </ApiProvider>
  ));
}

describe("DeliverableDetailPage", () => {
  it("无匹配 task → empty state", async () => {
    const { findByTestId } = setup({ deliverables: [] });
    await findByTestId("deliverable-detail-empty");
  });

  it("有 entries → 渲染 kind label + 文件 + 状态 + accept/reject 按钮", async () => {
    const { findByTestId, getAllByText } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "research_summary",
          files: [
            { name: "report.md", sha256: "abc1234567890def", size_bytes: 4096 },
          ],
          produced_at: "2026-05-02T10:00:00Z",
          status: "pending",
        },
      ],
    });
    const entry = await findByTestId(
      "deliverable-detail-entry-research_summary",
    );
    expect(entry.textContent).toContain("调研");
    expect(entry.textContent).toContain("report.md");
    expect(entry.textContent).toContain("4.0KB");
    // sha 短显（前 16 位）
    expect(entry.textContent).toContain("abc1234567890def");

    const status = await findByTestId("deliverable-detail-status-pending");
    expect(status.textContent).toContain("待处理");

    const accept = (await findByTestId(
      "deliverable-detail-accept",
    )) as HTMLButtonElement;
    expect(accept.disabled).toBe(false);
    expect(getAllByText("接收").length).toBeGreaterThan(0);
  });

  it("非 pending 状态 → 不渲染处理表单", async () => {
    const { findByTestId, queryByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "code_change",
          files: [{ name: "x.diff", sha256: "1234", size_bytes: 100 }],
          produced_at: "2026-05-02T10:00:00Z",
          status: "accepted",
        },
      ],
    });
    await findByTestId("deliverable-detail-status-accepted");
    expect(queryByTestId("deliverable-detail-accept")).toBeNull();
    expect(queryByTestId("deliverable-detail-reject")).toBeNull();
  });

  it("文件下载链接拼了 task- 前缀", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "code_change",
          files: [{ name: "patch.diff", sha256: "1234", size_bytes: 100 }],
          produced_at: "2026-05-02T10:00:00Z",
          status: "pending",
        },
      ],
    });
    const fileRow = await findByTestId(
      `deliverable-detail-file-${TASK_UUID}-patch.diff`,
    );
    const link = fileRow.querySelector("a") as HTMLAnchorElement;
    expect(link.href).toContain(`/api/deliverables/erp/task-${TASK_UUID}/files/patch.diff`);
  });
});
