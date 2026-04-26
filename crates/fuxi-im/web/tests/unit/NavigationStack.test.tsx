import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { NavigationStack } from "~/components/NavigationStack";

describe("NavigationStack", () => {
  it("top=undefined · 只渲 base", () => {
    const onPop = vi.fn();
    const { getByTestId, queryByTestId, unmount } = render(() => (
      <NavigationStack
        base={<div data-testid="base-content">base</div>}
        onPop={onPop}
      />
    ));
    expect(getByTestId("base-content")).toBeTruthy();
    expect(queryByTestId("nav-top")).toBeNull();
    unmount();
  });

  it("top=有节点 · 渲 base + top 两层", () => {
    const onPop = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <NavigationStack
        base={<div data-testid="base-content">base</div>}
        top={<div data-testid="top-content">top</div>}
        onPop={onPop}
      />
    ));
    expect(getByTestId("base-content")).toBeTruthy();
    expect(getByTestId("top-content")).toBeTruthy();
    expect(getByTestId("nav-top")).toBeTruthy();
    unmount();
  });

  it("ESC · 有 top 时 onPop 被调", () => {
    const onPop = vi.fn();
    const { unmount } = render(() => (
      <NavigationStack
        base={<div>base</div>}
        top={<div>top</div>}
        onPop={onPop}
      />
    ));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onPop).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("ESC · 没 top 时 onPop 不调", () => {
    const onPop = vi.fn();
    const { unmount } = render(() => (
      <NavigationStack base={<div>base</div>} onPop={onPop} />
    ));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onPop).not.toHaveBeenCalled();
    unmount();
  });

  it("edge swipe · 起点 ≤ EDGE_PX + 横滑超阈值 → onPop", () => {
    const onPop = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <NavigationStack
        base={<div>base</div>}
        top={<div>top</div>}
        onPop={onPop}
      />
    ));
    const top = getByTestId("nav-top");
    // jsdom clientWidth 0 → fallback 到 window.innerWidth ~1024
    fireEvent.touchStart(top, { touches: [{ clientX: 10, clientY: 400 }] });
    fireEvent.touchMove(top, { touches: [{ clientX: 600, clientY: 400 }] }); // dx=590
    fireEvent.touchEnd(top);
    expect(onPop).toHaveBeenCalledTimes(1);
    unmount();
  });

  it("edge swipe · 起点远离左缘 · 不触发", () => {
    const onPop = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <NavigationStack
        base={<div>base</div>}
        top={<div>top</div>}
        onPop={onPop}
      />
    ));
    const top = getByTestId("nav-top");
    fireEvent.touchStart(top, { touches: [{ clientX: 200, clientY: 400 }] });
    fireEvent.touchMove(top, { touches: [{ clientX: 800, clientY: 400 }] });
    fireEvent.touchEnd(top);
    expect(onPop).not.toHaveBeenCalled();
    unmount();
  });

  // 注：jsdom touch axis-lock 处理不稳，"未达阈值"路径在浏览器 e2e 才靠谱。
  // 这里只验主线 edge-from-left + ESC + 远离左缘不触发。
});
