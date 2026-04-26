import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { UserBubble } from "~/components/messages/UserBubble";
import type { UserMessage } from "~/messages";

const sample: UserMessage = {
  kind: "user",
  id: "u-1",
  text: "派活：修 ERP 客户列表",
  pending: false,
  ts: Date.now(),
};

describe("UserBubble", () => {
  it("渲染文本", () => {
    const { container, unmount } = render(() => <UserBubble msg={sample} />);
    expect(container.textContent).toContain("派活：修 ERP 客户列表");
    unmount();
  });

  it("error 时下挂红字 inline 错误", () => {
    const { getByTestId, unmount } = render(() => (
      <UserBubble msg={{ ...sample, error: "玄女后端不在" }} />
    ));
    expect(getByTestId("msg-user-error").textContent).toContain("玄女后端不在");
    unmount();
  });

  it("pending=true 时挂 pending 类（视觉压暗）", () => {
    const { getByTestId, unmount } = render(() => (
      <UserBubble msg={{ ...sample, pending: true }} />
    ));
    const el = getByTestId("msg-user");
    // bubble 子元素带 pending class（CSS module 名带哈希前缀，宽松断言）
    const bubble = el.querySelector("div");
    expect(bubble?.className).toMatch(/pending/);
    unmount();
  });
});
