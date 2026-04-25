import { describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { TalkToXuannvBar } from "~/components/TalkToXuannvBar";
import { createMockApi } from "../mocks/api";

describe("TalkToXuannvBar", () => {
  it("Enter（无 shift）触发 intervene 调用并清空输入", async () => {
    const api = createMockApi();
    setApiOverride(api);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider>
        <TalkToXuannvBar />
      </ApiProvider>
    ));
    const input = getByTestId("xuannv-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "看一下 ERP 任务进度" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(1);
    expect(api.state.intervenes[0]?.text).toBe("看一下 ERP 任务进度");
    expect(input.value).toBe("");
    setApiOverride(null);
    unmount();
  });

  it("Shift+Enter 不触发发送（保留换行行为）", async () => {
    const api = createMockApi();
    setApiOverride(api);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider>
        <TalkToXuannvBar />
      </ApiProvider>
    ));
    const input = getByTestId("xuannv-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "稍等" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.intervenes).toHaveLength(0);
    setApiOverride(null);
    unmount();
  });

  it("空字符串不发送，按钮禁用", async () => {
    const api = createMockApi();
    setApiOverride(api);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider>
        <TalkToXuannvBar />
      </ApiProvider>
    ));
    const send = getByTestId("xuannv-send") as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    setApiOverride(null);
    unmount();
  });
});
