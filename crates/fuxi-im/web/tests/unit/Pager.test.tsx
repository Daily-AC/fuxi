import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { Pager } from "~/components/Pager";

describe("Pager", () => {
  const labels = ["A", "B", "C"];
  const pages = (): Element[] => [
    document.createElement("div"),
    document.createElement("div"),
    document.createElement("div"),
  ];

  it("渲染 N 页 + N dots", () => {
    const { queryAllByTestId, unmount } = render(() => (
      <Pager
        pages={[<p>page0</p>, <p>page1</p>, <p>page2</p>]}
        index={0}
        onIndexChange={() => {}}
        pageLabels={labels}
      />
    ));
    expect(queryAllByTestId(/^pager-page-/)).toHaveLength(3);
    expect(queryAllByTestId(/^pager-dot-/)).toHaveLength(3);
    void pages;
    unmount();
  });

  it("当前 dot 标 aria-current=page", () => {
    const { getByTestId, unmount } = render(() => (
      <Pager
        pages={[<p>0</p>, <p>1</p>, <p>2</p>]}
        index={1}
        onIndexChange={() => {}}
        pageLabels={labels}
      />
    ));
    expect(getByTestId("pager-dot-1").getAttribute("aria-current")).toBe("page");
    expect(getByTestId("pager-dot-0").getAttribute("aria-current")).toBeNull();
    unmount();
  });

  it("点 dot · onIndexChange 被调", () => {
    const onChange = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <Pager
        pages={[<p>0</p>, <p>1</p>, <p>2</p>]}
        index={0}
        onIndexChange={onChange}
        pageLabels={labels}
      />
    ));
    fireEvent.click(getByTestId("pager-dot-2"));
    expect(onChange).toHaveBeenCalledWith(2);
    unmount();
  });

  it("touch 左滑超阈值 → onIndexChange(+1)", () => {
    const onChange = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <Pager
        pages={[<p>0</p>, <p>1</p>, <p>2</p>]}
        index={1}
        onIndexChange={onChange}
        pageLabels={labels}
      />
    ));
    const track = getByTestId("pager-track");
    // jsdom innerWidth 默认 1024 → threshold 18%
    fireEvent.touchStart(track, { touches: [{ clientX: 500, clientY: 400 }] });
    fireEvent.touchMove(track, { touches: [{ clientX: 200, clientY: 400 }] }); // dx=-300
    fireEvent.touchEnd(track);
    expect(onChange).toHaveBeenCalledWith(2);
    unmount();
  });

  it("touch 右滑超阈值 · 在 page 0 时不触发（边界）", () => {
    const onChange = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <Pager
        pages={[<p>0</p>, <p>1</p>, <p>2</p>]}
        index={0}
        onIndexChange={onChange}
        pageLabels={labels}
      />
    ));
    const track = getByTestId("pager-track");
    fireEvent.touchStart(track, { touches: [{ clientX: 100, clientY: 400 }] });
    fireEvent.touchMove(track, { touches: [{ clientX: 600, clientY: 400 }] });
    fireEvent.touchEnd(track);
    expect(onChange).not.toHaveBeenCalled();
    unmount();
  });

  // 注：jsdom touch event 处理不一致，axis-lock 路径在浏览器 e2e 才稳定验。
  // 这里只验主线 horizontal swipe 路径 + 边界 + dot click。
});
