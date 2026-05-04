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

  it("文件显式 ⬇ 下载按钮拼了 task- 前缀（用户主动触发下载）", async () => {
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
    const dl = (await findByTestId(
      `deliverable-detail-download-${TASK_UUID}-patch.diff`,
    )) as HTMLAnchorElement;
    expect(dl.href).toContain(`/api/deliverables/erp/task-${TASK_UUID}/files/patch.diff`);
    expect(dl.getAttribute("download")).toBe("patch.diff");
    // bug #77 改 icon-only：textContent 是 "⬇" 不再是 "下载"。aria-label/title 兜底语义。
    expect(dl.getAttribute("aria-label")).toContain("下载");
    expect(dl.getAttribute("title")).toBe("下载");
  });

  // bug #76：图片格式应显示 <img> 内联预览
  it("bug #76 · 图片文件 → <img> 内联预览（不需 fetch）", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "research_summary",
          files: [{ name: "diagram.png", sha256: "ff", size_bytes: 8192 }],
          produced_at: "2026-05-02T10:00:00Z",
          status: "pending",
        },
      ],
    });
    const img = (await findByTestId(
      `deliverable-detail-preview-${TASK_UUID}-diagram.png`,
    )) as HTMLImageElement;
    expect(img.tagName).toBe("IMG");
    expect(img.src).toContain(`/api/deliverables/erp/task-${TASK_UUID}/preview/diagram.png`);
    expect(img.alt).toBe("diagram.png");
  });

  // bug #76：未知格式（如 .bin）不预览，落 fallback
  it("bug #76 · 未支持格式 → 「请下载」fallback 文案", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "code_change",
          files: [{ name: "blob.bin", sha256: "ff", size_bytes: 1024 }],
          produced_at: "2026-05-02T10:00:00Z",
          status: "pending",
        },
      ],
    });
    const fb = await findByTestId(
      `deliverable-detail-no-preview-${TASK_UUID}-blob.bin`,
    );
    expect(fb.textContent).toContain("请下载");
  });

  // bug #76 · 大文本文件超 512KB 不内联预览（避免拉爆浏览器）
  it("bug #76 · 大文本文件 > 512KB → 不预览，提示下载", async () => {
    const { findByTestId } = setup({
      deliverables: [
        {
          project: "erp",
          task: TASK_UUID,
          kind: "code_change",
          files: [{ name: "big.log", sha256: "ff", size_bytes: 1024 * 1024 }],
          produced_at: "2026-05-02T10:00:00Z",
          status: "pending",
        },
      ],
    });
    const fb = await findByTestId(
      `deliverable-detail-no-preview-${TASK_UUID}-big.log`,
    );
    expect(fb.textContent).toMatch(/文件 > .+KB/);
  });
});
