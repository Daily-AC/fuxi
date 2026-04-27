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

  it("v3 #68 (a) · 短文本不竖排 · bubble 用 fit-content 宽度（CSS module 类命中）", () => {
    const { getByTestId, unmount } = render(() => (
      <UserBubble msg={{ ...sample, text: "hi" }} />
    ));
    const el = getByTestId("msg-user");
    const bubble = el.querySelector("div") as HTMLElement;
    // CSS Modules 名 hash 后包含 "bubble"——bubble 被 .bubble 样式覆盖
    expect(bubble?.className).toMatch(/bubble/);
    // 短文本"hi"应渲染在 bubble 内（不竖排）
    expect(bubble?.textContent).toContain("hi");
    // jsdom 不解析 CSS Modules 文件 → 无法直接验 width 计算值，靠 e2e 截图兜底
    // 单测层面只能验类名 + 文本不被切；视觉验证留给 e2e/手动
    unmount();
  });
});
