// MentionComposer 按住说话（PTT）——按下起 ASR、松手把识别文本填进输入框。
// VoiceController 本体在 voice-controller.test.ts 锁；这里只测 composer 接线。
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { MentionComposer } from "~/components/MentionComposer";
import { dismissToast } from "~/lib/toast";
import { createMockApi } from "../mocks/api";
import type { JSX } from "solid-js";

afterEach(() => {
  setApiOverride(null);
  dismissToast();
});

function renderWithApi(body: () => JSX.Element): ReturnType<typeof render> {
  setApiOverride(createMockApi());
  return render(() => <ApiProvider initialAuth="in">{body()}</ApiProvider>);
}

const noopSubmit = async (): Promise<void> => undefined;

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 10));

describe("MentionComposer · 按住说话", () => {
  it("不传 ptt prop 时不渲染 mic 按钮（其它使用方零影响）", () => {
    const { queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    expect(queryByTestId("composer-ptt")).toBeNull();
  });

  it("按下调 start、松手调 stop 并把文本追加进输入框", async () => {
    const start = vi.fn(async () => {});
    const stop = vi.fn(async () => "今天天气怎么样");
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} ptt={{ start, stop }} />
    ));
    const btn = getByTestId("composer-ptt");

    fireEvent.pointerDown(btn);
    await tick();
    expect(start).toHaveBeenCalledTimes(1);

    fireEvent.pointerUp(btn);
    await tick();
    expect(stop).toHaveBeenCalledTimes(1);
    expect((getByTestId("mention-editor") as HTMLTextAreaElement).value).toBe(
      "今天天气怎么样",
    );
  });

  it("已有文本时识别结果追加在末尾不覆盖", async () => {
    const start = vi.fn(async () => {});
    const stop = vi.fn(async () => "明天呢");
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} ptt={{ start, stop }} />
    ));
    const editor = getByTestId("mention-editor") as HTMLTextAreaElement;
    fireEvent.input(editor, { target: { value: "查天气，" } });

    fireEvent.pointerDown(getByTestId("composer-ptt"));
    await tick();
    fireEvent.pointerUp(getByTestId("composer-ptt"));
    await tick();
    expect(editor.value).toBe("查天气，明天呢");
  });

  it("stop 返回空串不动输入框", async () => {
    const start = vi.fn(async () => {});
    const stop = vi.fn(async () => "");
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} ptt={{ start, stop }} />
    ));
    fireEvent.pointerDown(getByTestId("composer-ptt"));
    await tick();
    fireEvent.pointerUp(getByTestId("composer-ptt"));
    await tick();
    expect((getByTestId("mention-editor") as HTMLTextAreaElement).value).toBe("");
  });

  it("pointercancel（来电/滑出）也走 stop，不留挂起录音", async () => {
    const start = vi.fn(async () => {});
    const stop = vi.fn(async () => "");
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} ptt={{ start, stop }} />
    ));
    fireEvent.pointerDown(getByTestId("composer-ptt"));
    await tick();
    fireEvent.pointerCancel(getByTestId("composer-ptt"));
    await tick();
    expect(stop).toHaveBeenCalledTimes(1);
  });
});
