import { describe, expect, it, vi } from "vitest";
import { render } from "@solidjs/testing-library";
import { StreamingText } from "~/components/StreamingText";

describe("StreamingText", () => {
  it("最终渲染完整文本", async () => {
    const { container, unmount } = render(() => (
      <StreamingText text="玄女在听" charDelay={1} />
    ));
    // 字符级动画完成后 textContent 应该等于完整文本
    await new Promise((r) => setTimeout(r, 60));
    expect(container.textContent ?? "").toContain("玄女在听");
    unmount();
  });

  it("streaming=true 时显示尾部光标", async () => {
    const { container, unmount } = render(() => (
      <StreamingText text="hello" streaming charDelay={1} />
    ));
    await new Promise((r) => setTimeout(r, 60));
    expect(container.querySelector(".stream-cursor")).toBeTruthy();
    unmount();
  });

  it("prefers-reduced-motion 时一次性渲染（无逐字延迟）", async () => {
    const orig = window.matchMedia;
    window.matchMedia = vi.fn().mockImplementation((q: string) => ({
      matches: q.includes("reduce"),
      media: q,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
    const { container, unmount } = render(() => (
      <StreamingText text="immediate" charDelay={9999} />
    ));
    // 不等 charDelay，立即检查
    expect(container.textContent ?? "").toContain("immediate");
    window.matchMedia = orig;
    unmount();
  });
});
