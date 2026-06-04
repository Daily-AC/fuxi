import { describe, expect, it, vi } from "vitest";
import { render, fireEvent } from "@solidjs/testing-library";
import { Mascot } from "~/components/Mascot/Mascot";

describe("Mascot", () => {
  it("按 state 渲染对应帧 img（src 含 state 名）", () => {
    const { getByTestId, unmount } = render(() => <Mascot state="happy" size={120} />);
    const img = getByTestId("mascot-img") as HTMLImageElement;
    expect(img.getAttribute("src")).toContain("xuannv-happy");
    unmount();
  });
  it("可戳：点击触发 onPoke", () => {
    const onPoke = vi.fn();
    const { getByTestId, unmount } = render(() => <Mascot state="idle" size={120} onPoke={onPoke} />);
    fireEvent.click(getByTestId("mascot"));
    expect(onPoke).toHaveBeenCalledOnce();
    unmount();
  });
});
