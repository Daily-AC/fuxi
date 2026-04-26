import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { FileMessage } from "~/components/messages/FileMessage";
import type { FileMessage as FileMsg } from "~/messages";

const sample: FileMsg = {
  kind: "file",
  id: "f-1",
  role: "user",
  caption: "看 **这个**",
  attachments: [
    { id: "u-1", name: "a.png", mime: "image/png", bytes: 100, sha256: "" },
    { id: "u-2", name: "b.pdf", mime: "application/pdf", bytes: 1000, sha256: "" },
  ],
  ts: Date.now(),
};

describe("FileMessage", () => {
  it("渲染 caption (markdown) + 多个 chip", () => {
    const { container, getByTestId, unmount } = render(() => <FileMessage msg={sample} />);
    expect(container.querySelector("strong")?.textContent).toBe("这个");
    expect(getByTestId("attachment-chip-u-1")).toBeTruthy();
    expect(getByTestId("attachment-chip-u-2")).toBeTruthy();
    unmount();
  });

  it("user role · data-role=user，bubble 在右", () => {
    const { getByTestId, unmount } = render(() => <FileMessage msg={sample} />);
    const el = getByTestId("msg-file");
    expect(el.getAttribute("data-role")).toBe("user");
    unmount();
  });

  it("agent role · 不在右对齐", () => {
    const xn: FileMsg = { ...sample, role: "xuannv", caption: undefined };
    const { getByTestId, unmount } = render(() => <FileMessage msg={xn} />);
    const el = getByTestId("msg-file");
    expect(el.getAttribute("data-role")).toBe("xuannv");
    unmount();
  });
});
