import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { MentionComposer } from "~/components/MentionComposer";
import { Toast } from "~/components/Toast";
import { dismissToast } from "~/lib/toast";
import type { MentionCandidate, SerializedIntervene } from "~/lib/mentions";
import { createMockApi } from "../mocks/api";
import type { JSX } from "solid-js";

afterEach(() => {
  setApiOverride(null);
  dismissToast();
});

/** v3 #46 起 MentionComposer 内部 useApi() 拿 client.uploadFile，必须在 ApiProvider 下渲染。 */
function renderWithApi(body: () => JSX.Element): ReturnType<typeof render> {
  setApiOverride(createMockApi());
  return render(() => <ApiProvider initialAuth="in">{body()}</ApiProvider>);
}

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
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={onSubmit} />
    ));
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(true);
  });

  it("输入文本 · send enable + 提交后 reset", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = vi.fn().mockImplementation(async (req: SerializedIntervene) => {
      captured = req;
    });
    const { getByTestId } = renderWithApi(() => (
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
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN, PUSONG]} onSubmit={noopSubmit} />
    ));
    expect(queryByTestId("mention-popup")).toBeNull();
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "查 @" } });
    expect(getByTestId("mention-popup")).toBeTruthy();
    expect(getByTestId("mention-item-a-luban")).toBeTruthy();
  });

  it("输 @lu 过滤候选 · 仅鲁班", () => {
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN, PUSONG]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    expect(queryByTestId("mention-item-a-luban")).toBeTruthy();
    expect(queryByTestId("mention-item-a-pusong")).toBeNull();
  });

  it("选候选 → 文本 @query 部分被替换为 chip · @ 后空 text 段", () => {
    const { getByTestId, queryByTestId } = renderWithApi(() => (
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
    const { getByTestId } = renderWithApi(() => (
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
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    expect(getByTestId("mention-chip-a-luban")).toBeTruthy();
    fireEvent.click(getByTestId("mention-chip-remove-a-luban"));
    expect(queryByTestId("mention-chip-a-luban")).toBeNull();
  });

  it("Backspace 在末段 text 为空时 · 删前一个 chip", () => {
    const { getByTestId, queryByTestId } = renderWithApi(() => (
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
    const { getByTestId, queryByTestId } = renderWithApi(() => (
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
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} placeholder="对鲁班说..." onSubmit={noopSubmit} />
    ));
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    expect(editor.placeholder).toBe("对鲁班说...");
  });

  it("@ 后输空格 · 关 autocomplete", () => {
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@" } });
    expect(queryByTestId("mention-popup")).toBeTruthy();
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@ " } });
    expect(queryByTestId("mention-popup")).toBeNull();
  });

  it("候选过滤后空集 · 显空态文案", () => {
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@xxxxx" } });
    expect(getByTestId("mention-popup-empty").textContent).toContain("没找到");
  });

  // ===== 附件 +/upload v3 #46 =====

  it("+ 按钮存在 · 选文件 → attach chip 出现 · 文件名 + 大小", () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    expect(getByTestId("composer-attach-btn")).toBeTruthy();
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["hello"], "screenshot.png", { type: "image/png" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    const chips = queryAllByTestId(/^composer-attach-c-/);
    expect(chips).toHaveLength(1);
    expect(chips[0]?.textContent).toContain("screenshot.png");
  });

  it("有附件 + 空 text · send enable（纯附件消息）", () => {
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(true);
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(false);
  });

  it("发送 → uploadFile 调用 + onSubmit req.attachments=[upload.id]", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = async (req: SerializedIntervene): Promise<void> => {
      captured = req;
    };
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={onSubmit} />
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["data"], "doc.pdf", { type: "application/pdf" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "看看这个" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 50));
    expect(captured?.text).toBe("看看这个");
    expect(captured?.attachments).toBeTruthy();
    expect(captured?.attachments?.length).toBe(1);
    // mock api uploadFile 返 up-1
    expect(captured?.attachments?.[0]).toMatch(/^up-/);
  });

  it("✕ 删除 attach chip · chip 下线", () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    const chips = queryAllByTestId(/^composer-attach-c-/);
    expect(chips).toHaveLength(1);
    const cid = chips[0]?.getAttribute("data-testid")?.replace("composer-attach-", "") ?? "";
    fireEvent.click(getByTestId(`composer-attach-remove-${cid}`));
    expect(queryAllByTestId(/^composer-attach-c-/)).toHaveLength(0);
  });

  it("发送后 reset · attach chips 清空", async () => {
    const onSubmit = async (): Promise<void> => undefined;
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={onSubmit} />
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 50));
    expect(queryAllByTestId(/^composer-attach-c-/)).toHaveLength(0);
  });

  it("Enter 在 query 模式不触发 send（autocomplete 接管 Enter）", async () => {
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId } = renderWithApi(() => (
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
