import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { Conversation } from "~/views/Conversation";

describe("Conversation", () => {
  it("空 messages → 渲染空态'玄女在线 · 跟她说点啥'", () => {
    const [msgs] = createSignal<unknown[]>([]);
    const { getByTestId, unmount } = render(() => <Conversation messages={msgs} />);
    const empty = getByTestId("conversation-empty");
    expect(empty.textContent).toContain("玄女在线");
    expect(empty.textContent).toContain("跟她说点啥");
    unmount();
  });

  it("非空 messages → 渲染 stream 容器（不显示空态）", () => {
    const [msgs] = createSignal<unknown[]>([{ kind: "stub" }]);
    const { getByTestId, queryByTestId, unmount } = render(() => (
      <Conversation messages={msgs} />
    ));
    expect(getByTestId("conversation-stream")).toBeTruthy();
    expect(queryByTestId("conversation-empty")).toBeNull();
    unmount();
  });
});
