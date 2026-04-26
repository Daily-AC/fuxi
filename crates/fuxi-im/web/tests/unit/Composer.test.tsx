import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { Composer } from "~/components/Composer";

describe("Composer", () => {
  it("空字符串按钮 disabled，键入后 active", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => <Composer onSubmit={onSubmit} />);
    const btn = getByTestId("composer-send") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    expect(btn.disabled).toBe(false);
    unmount();
  });

  it("Enter 提交（无 shift），清空输入", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => <Composer onSubmit={onSubmit} />);
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "派活：修 ERP" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await new Promise((r) => setTimeout(r, 30));
    expect(onSubmit).toHaveBeenCalledWith("派活：修 ERP");
    expect(input.value).toBe("");
    unmount();
  });

  it("Shift+Enter 不提交（保留换行）", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => <Composer onSubmit={onSubmit} />);
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "等下" } });
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    await new Promise((r) => setTimeout(r, 30));
    expect(onSubmit).not.toHaveBeenCalled();
    unmount();
  });

  it("submitting 期 disabled 防双击", async () => {
    let resolveFn: () => void = () => {};
    const onSubmit = vi.fn().mockImplementation(
      () =>
        new Promise<void>((r) => {
          resolveFn = r;
        }),
    );
    const { getByTestId, unmount } = render(() => <Composer onSubmit={onSubmit} />);
    const input = getByTestId("composer-input") as HTMLTextAreaElement;
    fireEvent.input(input, { target: { value: "x" } });
    fireEvent.click(getByTestId("composer-send"));
    await new Promise((r) => setTimeout(r, 0));
    expect((getByTestId("composer-send") as HTMLButtonElement).disabled).toBe(true);
    expect(input.disabled).toBe(true);
    resolveFn();
    unmount();
  });

  it("disabled prop 强制禁用（如 ws 未就绪）", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, unmount } = render(() => <Composer onSubmit={onSubmit} disabled />);
    fireEvent.input(getByTestId("composer-input"), { target: { value: "hi" } });
    expect((getByTestId("composer-send") as HTMLButtonElement).disabled).toBe(true);
    unmount();
  });
});
