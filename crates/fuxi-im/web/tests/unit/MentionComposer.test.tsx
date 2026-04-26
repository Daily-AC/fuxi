import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { MentionComposer } from "~/components/MentionComposer";
import { Toast } from "~/components/Toast";
import { dismissToast } from "~/lib/toast";
import type { MentionCandidate, SerializedIntervene } from "~/lib/mentions";

afterEach(() => {
  dismissToast();
});

const LUBAN: MentionCandidate = {
  agent_id: "a-luban",
  role: "luban",
  role_display: "鲁班",
  hint: "grep server",
};
const PUSONG: MentionCandidate = {
  agent_id: "a-pusong",
  role: "pusong",
  role_display: "蒲松",
  hint: "待命",
};

// 单独抽出来避免 solid/reactivity lint 误把 inline async () => undefined 当 tracked scope
const noopSubmit = async (): Promise<void> => undefined;

describe("MentionComposer · v3 #N5'/#N4'", () => {
  it("空输入 · send 按钮 disabled", () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={onSubmit} />
    ));
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(true);
  });

  it("输入文本 · send enable + 提交后 reset", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = vi.fn().mockImplementation(async (req: SerializedIntervene) => {
      captured = req;
    });
    const { getByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={onSubmit} />
    ));
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    fireEvent.input(editor, { target: { value: "你好玄女" } });
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(onSubmit).toHaveBeenCalled();
    expect(captured?.text).toBe("你好玄女");
    expect(captured?.target).toBeUndefined();
    expect(captured?.mentions).toEqual([]);
  });

  it("输 @ 触发 autocomplete 弹层", () => {
    const { getByTestId, queryByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN, PUSONG]} onSubmit={noopSubmit} />
    ));
    expect(queryByTestId("mention-popup")).toBeNull();
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "查 @" } });
    expect(getByTestId("mention-popup")).toBeTruthy();
    expect(getByTestId("mention-item-a-luban")).toBeTruthy();
  });

  it("输 @lu 过滤候选 · 仅鲁班", () => {
    const { getByTestId, queryByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN, PUSONG]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    expect(queryByTestId("mention-item-a-luban")).toBeTruthy();
    expect(queryByTestId("mention-item-a-pusong")).toBeNull();
  });

  it("选候选 → 文本 @query 部分被替换为 chip · @ 后空 text 段", () => {
    const { getByTestId, queryByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "查 @lu" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    expect(queryByTestId("mention-popup")).toBeNull();
    // chip 上方一行渲染
    expect(getByTestId("mention-chip-a-luban")).toBeTruthy();
    // 末段 text 由 chip+空格 组成（input value 是末段 text）
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    expect(editor.value).toBe(" ");
  });

  it("选候选后发送 · target = chip.agent_id + mentions[a-luban]", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = async (req: SerializedIntervene): Promise<void> => {
      captured = req;
    };
    const { getByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={onSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 查 ERP" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(captured?.target).toBe("a-luban");
    expect(captured?.mentions).toEqual(["a-luban"]);
    expect(captured?.text).toContain("查 ERP");
  });

  it("chip ✕ 删除 · chip 下线", () => {
    const { getByTestId, queryByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    expect(getByTestId("mention-chip-a-luban")).toBeTruthy();
    fireEvent.click(getByTestId("mention-chip-remove-a-luban"));
    expect(queryByTestId("mention-chip-a-luban")).toBeNull();
  });

  it("Backspace 在末段 text 为空时 · 删前一个 chip", () => {
    const { getByTestId, queryByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    // 末段是 " "（chip 后默认插的空格），先清空
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    fireEvent.input(editor, { target: { value: "" } });
    expect(getByTestId("mention-chip-a-luban")).toBeTruthy();
    fireEvent.keyDown(editor, { key: "Backspace" });
    expect(queryByTestId("mention-chip-a-luban")).toBeNull();
  });

  it("两个 chip · 发送时 toast 多 chip 警示", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryByTestId } = render(() => (
      <>
        <MentionComposer candidates={[LUBAN, PUSONG]} onSubmit={onSubmit} />
        <Toast />
      </>
    ));
    // 加 chip1 鲁班
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    // 加 chip2 蒲松
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " @pu" } });
    fireEvent.click(getByTestId("mention-item-a-pusong"));
    // 发送
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 一起干" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(onSubmit).toHaveBeenCalled();
    expect(queryByTestId("toast")).toBeTruthy();
    expect(queryByTestId("toast")?.textContent).toContain("第一个");
  });

  it("placeholder 透传", () => {
    const { getByTestId } = render(() => (
      <MentionComposer candidates={[]} placeholder="对鲁班说..." onSubmit={noopSubmit} />
    ));
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    expect(editor.placeholder).toBe("对鲁班说...");
  });

  it("@ 后输空格 · 关 autocomplete", () => {
    const { getByTestId, queryByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    expect(queryByTestId("mention-popup")).toBeTruthy();
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@ " } });
    expect(queryByTestId("mention-popup")).toBeNull();
  });

  it("候选过滤后空集 · 显空态文案", () => {
    const { getByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@xxxxx" } });
    expect(getByTestId("mention-popup-empty").textContent).toContain("没找到");
  });

  it("Enter 在 query 模式不触发 send（autocomplete 接管 Enter）", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId } = render(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={onSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "查 @lu" } });
    // Enter 此时由 MentionAutocomplete document keydown 接管 → 选中候选
    fireEvent.keyDown(document, { key: "Enter" });
    await new Promise((r) => setTimeout(r, 10));
    expect(getByTestId("mention-chip-a-luban")).toBeTruthy();
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
