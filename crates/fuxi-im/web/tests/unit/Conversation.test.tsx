import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { Conversation } from "~/views/Conversation";
import type { Message, UserMessage, XuannvMessage } from "~/messages";

const userMsg: UserMessage = {
  kind: "user",
  id: "u-1",
  text: "派活：修 ERP",
  pending: false,
  ts: Date.now(),
};

const xuannvMsg: XuannvMessage = {
  kind: "xuannv",
  id: "xn-1",
  agent: "xuannv",
  text: "好的，我看一下",
  streaming: false,
  ts: Date.now(),
};

describe("Conversation", () => {
  it("空 messages → 渲染空态'玄女在线 · 跟她说点啥'", () => {
    const [msgs] = createSignal<Message[]>([]);
    const { getByTestId, unmount } = render(() => <Conversation messages={msgs} />);
    const empty = getByTestId("conversation-empty");
    expect(empty.textContent).toContain("玄女在线");
    expect(empty.textContent).toContain("跟她说点啥");
    unmount();
  });

  it("user + xuannv 消息混合 → 渲染两条 bubble", () => {
    const [msgs] = createSignal<Message[]>([userMsg, xuannvMsg]);
    const { getByTestId, queryByTestId, unmount } = render(() => (
      <Conversation messages={msgs} />
    ));
    expect(getByTestId("conversation-stream")).toBeTruthy();
    expect(getByTestId("msg-user").textContent).toContain("派活：修 ERP");
    expect(getByTestId("msg-xuannv").textContent).toContain("好的，我看一下");
    expect(queryByTestId("conversation-empty")).toBeNull();
    unmount();
  });
});
