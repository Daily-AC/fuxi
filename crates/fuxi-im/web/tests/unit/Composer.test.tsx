import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { Composer } from "~/components/Composer";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { createMockApi } from "../mocks/api";
import type { Upload } from "~/types/api";

afterEach(() => setApiOverride(null));

function setup(opts?: { uploadFail?: boolean }) {
  const api = createMockApi({ uploadFail: opts?.uploadFail });
  setApiOverride(api);
  return { api };
}

function makeFile(name: string, mime = "text/plain", size = 5): File {
  return new File([new Uint8Array(size)], name, { type: mime });
}

describe("Composer", () => {
  it("空字符串 + 无附件 → 按钮 disabled；键入后 active", () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const btn = getByTestId("composer-send") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    expect(btn.disabled).toBe(false);
    unmount();
  });

  it("Enter 提交（无 shift） · onSubmit 收到 (text, [])", async () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "派活：修 ERP" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await new Promise((r) => setTimeout(r, 30));
    expect(onSubmit).toHaveBeenCalledWith("派活：修 ERP", []);
    expect(input.value).toBe("");
    unmount();
  });

  it("Shift+Enter 不提交", async () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "等下" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    await new Promise((r) => setTimeout(r, 30));
    expect(onSubmit).not.toHaveBeenCalled();
    unmount();
  });

  it("disabled prop 强制禁用", () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} disabled />
      </ApiProvider>
    ));
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    expect((getByTestId("composer-send") as HTMLButtonElement).disabled).toBe(true);
    unmount();
  });

  it("选 2 文件 → chip 渲染 idle 状态", async () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const fi = getByTestId("composer-file-input") as HTMLInputElement;
    const files = [makeFile("a.txt"), makeFile("b.png", "image/png", 100)];
    fireEvent.change(fi, { target: { files } });
    await new Promise((r) => setTimeout(r, 10));
    const chips = queryAllByTestId(/^composer-chip-c-/);
    expect(chips).toHaveLength(2);
    expect(chips[0]?.getAttribute("data-status")).toBe("idle");
    unmount();
  });

  it("纯附件提交 · 无 text → 上传 → onSubmit 收到 (\"\", uploads[])", async () => {
    const { api } = setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const fi = getByTestId("composer-file-input") as HTMLInputElement;
    fireEvent.change(fi, { target: { files: [makeFile("a.txt")] } });
    await new Promise((r) => setTimeout(r, 10));
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 50));
    expect(api.state.uploads).toHaveLength(1);
    expect(onSubmit).toHaveBeenCalledTimes(1);
    const [text, ups] = onSubmit.mock.calls[0] as [string, Upload[]];
    expect(text).toBe("");
    expect(ups).toHaveLength(1);
    expect(ups[0]?.id).toBe("up-1");
    // 提交完 chip 清空
    expect(queryAllByTestId(/^composer-chip-c-/)).toHaveLength(0);
    unmount();
  });

  it("上传失败 → chip 进 error 状态 + 不调 onSubmit", async () => {
    setup({ uploadFail: true });
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const fi = getByTestId("composer-file-input") as HTMLInputElement;
    fireEvent.change(fi, { target: { files: [makeFile("bad.txt")] } });
    await new Promise((r) => setTimeout(r, 10));
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 50));
    expect(onSubmit).not.toHaveBeenCalled();
    const chips = queryAllByTestId(/^composer-chip-c-/);
    expect(chips[0]?.getAttribute("data-status")).toBe("error");
    unmount();
  });

  // v1-session19 #1 · 粘贴自动附件 ——
  it("粘贴含 file 的剪贴板 → 自动起 chip", () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    const evt = new Event("paste", { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { files: File[] };
    };
    evt.clipboardData = { files: [makeFile("Screenshot.png", "image/png", 100)] };
    input.dispatchEvent(evt);
    expect(queryAllByTestId(/^composer-chip-c-/)).toHaveLength(1);
    expect(queryAllByTestId(/^composer-chip-c-/)[0]?.textContent).toContain("Screenshot.png");
    unmount();
  });

  it("粘贴纯文本 · 不当附件", () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    const evt = new Event("paste", { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { files: File[] };
    };
    evt.clipboardData = { files: [] };
    input.dispatchEvent(evt);
    expect(queryAllByTestId(/^composer-chip-c-/)).toHaveLength(0);
    unmount();
  });

  it("chip × 按钮 · 移除附件后按钮回 disabled（无 text 无 chip）", async () => {
    setup();
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryAllByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Composer onSubmit={onSubmit} />
      </ApiProvider>
    ));
    const fi = getByTestId("composer-file-input") as HTMLInputElement;
    fireEvent.change(fi, { target: { files: [makeFile("a.txt")] } });
    await new Promise((r) => setTimeout(r, 10));
    const chip = queryAllByTestId(/^composer-chip-c-/)[0];
    const cid = chip?.getAttribute("data-testid")?.replace("composer-chip-", "");
    expect(cid).toBeTruthy();
    fireEvent.click(getByTestId(`composer-chip-remove-${cid}`));
    await new Promise((r) => setTimeout(r, 0));
    expect(queryAllByTestId(/^composer-chip-c-/)).toHaveLength(0);
    expect((getByTestId("composer-send") as HTMLButtonElement).disabled).toBe(true);
    unmount();
  });
});
