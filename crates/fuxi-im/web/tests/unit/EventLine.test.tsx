import { describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { EventLine } from "~/components/EventLine";

describe("EventLine", () => {
  it("user_message 渲染'你' + 文字", () => {
    const { container, unmount } = render(() => (
      <EventLine ev={{ type: "user_message", text: "派活：修 ERP" }} />
    ));
    expect(container.textContent ?? "").toContain("你");
    expect(container.textContent ?? "").toContain("派活：修 ERP");
    unmount();
  });

  it("agent_responded 字符级流式渲染最终落到完整文本", async () => {
    const { container, unmount } = render(() => (
      <EventLine ev={{ type: "agent_responded", agent: "cc-1234abcd5678", text: "好的" }} />
    ));
    await new Promise((r) => setTimeout(r, 80));
    expect(container.textContent ?? "").toContain("好的");
    expect(container.querySelector(".agent-id")?.textContent).toContain("cc-1");
    unmount();
  });

  it("tool_call 默认折叠，点击展开", async () => {
    const { container, unmount } = render(() => (
      <EventLine
        ev={{
          type: "tool_call",
          agent: "cc-1234abcd5678",
          tool: "Read",
          input: { file_path: "/erp/pagination.ts" },
        }}
      />
    ));
    expect(container.querySelector("pre")).toBeFalsy();
    const head = container.querySelector("button") as HTMLButtonElement;
    fireEvent.click(head);
    await new Promise((r) => setTimeout(r, 0));
    expect(container.querySelector("pre")?.textContent).toContain("/erp/pagination.ts");
    unmount();
  });
});
