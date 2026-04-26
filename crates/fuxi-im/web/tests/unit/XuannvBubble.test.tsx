import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { XuannvBubble } from "~/components/messages/XuannvBubble";
import type { XuannvMessage } from "~/messages";

const base: XuannvMessage = {
  kind: "xuannv",
  id: "xn-1",
  agent: "xuannv",
  text: "好的，我看一下",
  streaming: false,
  ts: Date.UTC(2026, 3, 26, 12, 30),
};

describe("XuannvBubble", () => {
  it("渲染玄女 name + text", () => {
    const { container, unmount } = render(() => <XuannvBubble msg={base} />);
    expect(container.textContent).toContain("玄女");
    expect(container.textContent).toContain("好的，我看一下");
    unmount();
  });

  it("streaming=true 时挂 pulse-dot 不显示时间", () => {
    const { container, queryByTestId, unmount } = render(() => (
      <XuannvBubble msg={{ ...base, streaming: true }} />
    ));
    expect(queryByTestId("msg-streaming")).toBeTruthy();
    expect(container.querySelector(".pulse-dot")).toBeTruthy();
    unmount();
  });

  it("streaming=false 时不挂 pulse-dot 但显示时间", () => {
    const { queryByTestId, container, unmount } = render(() => <XuannvBubble msg={base} />);
    expect(queryByTestId("msg-streaming")).toBeNull();
    // 时间用 HH:mm 格式渲染（local time，不强断言具体值）
    expect(container.querySelector("time")).toBeTruthy();
    unmount();
  });
});
