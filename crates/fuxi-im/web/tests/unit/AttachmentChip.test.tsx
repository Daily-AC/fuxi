import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
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

  it("非 image · file icon + 文件名 + bytes", () => {
    const { container, unmount } = render(() => <AttachmentChip upload={fileUp} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("svg")).toBeTruthy();
    expect(container.textContent).toContain("report.pdf");
    expect(container.textContent).toContain("MB");
    unmount();
  });
});
