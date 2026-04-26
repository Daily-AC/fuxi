import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { BottomSheet } from "~/components/BottomSheet";

describe("BottomSheet", () => {
  it("open=false · 不渲染 panel", () => {
    const onClose = vi.fn();
    const { queryByTestId, unmount } = render(() => (
      <BottomSheet open={false} onClose={onClose} title="任务" testId="tasks-sheet">
        <p>body</p>
      </BottomSheet>
    ));
    expect(queryByTestId("tasks-sheet")).toBeNull();
    unmount();
  });

  it("open=true · 渲染 panel + handle + body", () => {
    const onClose = vi.fn();
    const { getByTestId, container, unmount } = render(() => (
      <BottomSheet open onClose={onClose} title="任务" testId="tasks-sheet">
        <p data-testid="body">body</p>
      </BottomSheet>
    ));
    expect(getByTestId("tasks-sheet")).toBeTruthy();
    expect(getByTestId("body")).toBeTruthy();
    expect(container.textContent).toContain("任务");
    unmount();
  });

  it("背景点击 → onClose", () => {
    const onClose = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <BottomSheet open onClose={onClose} title="任务" testId="tasks-sheet">
        body
      </BottomSheet>
    ));
    fireEvent.click(getByTestId("tasks-sheet-backdrop"));
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("关闭按钮 · onClose", () => {
    const onClose = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <BottomSheet open onClose={onClose} title="任务" testId="tasks-sheet">
        body
      </BottomSheet>
    ));
    fireEvent.click(getByTestId("tasks-sheet-close"));
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("ESC · onClose", () => {
    const onClose = vi.fn();
    const { unmount } = render(() => (
      <BottomSheet open onClose={onClose} title="任务" testId="tasks-sheet">
        body
      </BottomSheet>
    ));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("ESC 在 closed 状态 · 不调 onClose（无副作用）", () => {
    const onClose = vi.fn();
    const { unmount } = render(() => (
      <BottomSheet open={false} onClose={onClose} title="任务" testId="tasks-sheet">
        body
      </BottomSheet>
    ));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    unmount();
  });

  it("touch 下拉 ≥80px · onClose", () => {
    const onClose = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <BottomSheet open onClose={onClose} title="任务" testId="tasks-sheet">
        body
      </BottomSheet>
    ));
    const panel = getByTestId("tasks-sheet");
    fireEvent.touchStart(panel, { touches: [{ clientY: 100 }] });
    fireEvent.touchMove(panel, { touches: [{ clientY: 200 }] }); // dy=100
    fireEvent.touchEnd(panel);
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("touch 下拉 <80px · 不关", () => {
    const onClose = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <BottomSheet open onClose={onClose} title="任务" testId="tasks-sheet">
        body
      </BottomSheet>
    ));
    const panel = getByTestId("tasks-sheet");
    fireEvent.touchStart(panel, { touches: [{ clientY: 100 }] });
    fireEvent.touchMove(panel, { touches: [{ clientY: 130 }] }); // dy=30
    fireEvent.touchEnd(panel);
    expect(onClose).not.toHaveBeenCalled();
    unmount();
  });
});
