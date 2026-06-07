import { describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { AttachmentChip } from "~/components/messages/AttachmentChip";
import type { Upload } from "~/types/api";

const imageUp: Upload = {
  id: "up-1",
  name: "screenshot.png",
  mime: "image/png",
  bytes: 132456,
  sha256: "sha",
};

const fileUp: Upload = {
  id: "up-2",
  name: "report.pdf",
  mime: "application/pdf",
  bytes: 2_345_678,
  sha256: "sha",
};

describe("AttachmentChip", () => {
  it("image · 缩略图 + 文件名 + 直链", () => {
    const { container, unmount } = render(() => <AttachmentChip upload={imageUp} />);
    const img = container.querySelector("img");
    expect(img?.getAttribute("src")).toBe("/api/uploads/up-1");
    const a = container.querySelector("a");
    expect(a?.getAttribute("href")).toBe("/api/uploads/up-1");
    expect(a?.getAttribute("target")).toBe("_blank");
    expect(a?.getAttribute("rel")).toBe("noopener noreferrer");
    expect(container.textContent).toContain("screenshot.png");
    unmount();
  });

  // issue 3b5b8f25：点图片 chip 打开 in-app lightbox（fit-to-screen），
  // 不再走 WebView 原生 1:1 渲染（只见左上角）。
  it("image · 点击打开 lightbox viewer，img 用 contain fit", () => {
    const { getByTestId, unmount } = render(() => <AttachmentChip upload={imageUp} />);
    // viewer 经 Portal 挂到 document.body，作用域外，用 document 查询。
    const qViewer = () => document.body.querySelector('[data-testid="image-viewer"]');
    expect(qViewer()).toBeNull();

    fireEvent.click(getByTestId(`attachment-chip-${imageUp.id}`));

    const viewer = qViewer();
    expect(viewer).toBeTruthy();
    // viewer 内大图指向同一直链；fit-to-screen 由 ImageViewer.module.css 的
    // object-fit:contain 保证（这里断言 viewer 已挂载、大图 src 正确）。
    expect(viewer?.querySelector("img")?.getAttribute("src")).toBe("/api/uploads/up-1");

    // 关闭按钮收起 viewer
    const close = document.body.querySelector(
      '[data-testid="image-viewer-close"]',
    ) as HTMLElement;
    fireEvent.click(close);
    expect(qViewer()).toBeNull();
    unmount();
  });

  it("非 image · file icon + 文件名 + bytes", () => {
    const { container, unmount } = render(() => <AttachmentChip upload={fileUp} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).toBeTruthy();
    expect(container.textContent).toContain("report.pdf");
    expect(container.textContent).toContain("MB");
    unmount();
  });
});
