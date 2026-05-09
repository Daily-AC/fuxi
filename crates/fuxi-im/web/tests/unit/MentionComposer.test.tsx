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

  // ===== 附件 +/upload v3 #46 + #50 改"选完立即并发上传" =====

  it("+ 按钮存在 · 选文件 → attach chip 立即出现 · 上传完成后 status=done", async () => {
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
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId(/^composer-attach-c-/)[0]?.getAttribute("data-status")).toBe("done");
  });

  it("有附件 · 选完后 send 始终 enable（uploading 也可点 → toast；done 后真发）", async () => {
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(true);
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    // 即使 uploading 也 enable（spec §"否则禁 send + toast" 走 toast 而非 disable）
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(false);
    await new Promise((r) => setTimeout(r, 30));
    // done 后仍 enable
    expect((getByTestId("mention-send") as HTMLButtonElement).disabled).toBe(false);
  });

  it("发送 → uploadFile 调用 + onSubmit req.attachments=[upload.id]（选完立即上传）", async () => {
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
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "看看这个" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(captured?.text).toBe("看看这个");
    expect(captured?.attachments?.length).toBe(1);
    expect(captured?.attachments?.[0]).toMatch(/^up-/);
  });

  it("✕ 删除 attach chip (done) · chip 下线", async () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    await new Promise((r) => setTimeout(r, 30));
    const chips = queryAllByTestId(/^composer-attach-c-/);
    expect(chips).toHaveLength(1);
    const cid = chips[0]?.getAttribute("data-testid")?.replace("composer-attach-", "") ?? "";
    fireEvent.click(getByTestId(`composer-attach-remove-${cid}`));
    expect(queryAllByTestId(/^composer-attach-c-/)).toHaveLength(0);
  });

  // v1-session19 #1 · 粘贴自动附件 ——
  // 截图（Cmd+Shift+4 拷到剪贴板后 Cmd+V）/ Finder 复制文件 / drag-emul 都走
  // clipboardData.files；本测覆盖最直观的截图路径。
  it("v1-session19 · 粘贴含 file 的剪贴板 → 自动起 attach chip 并上传", async () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    const file = new File(["png-bytes"], "Screenshot.png", { type: "image/png" });
    // jsdom 不真造 ClipboardEvent.clipboardData.files；构造一个 mock 事件并 dispatch。
    // fireEvent.paste 会让 React/Solid 走 onPaste handler；clipboardData 我们手动塞。
    const evt = new Event("paste", { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { files: File[] };
    };
    evt.clipboardData = { files: [file] };
    editor.dispatchEvent(evt);
    const chips = queryAllByTestId(/^composer-attach-c-/);
    expect(chips).toHaveLength(1);
    expect(chips[0]?.textContent).toContain("Screenshot.png");
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId(/^composer-attach-c-/)[0]?.getAttribute("data-status")).toBe("done");
  });

  it("v1-session19 · 粘贴纯文本 · 不当附件（让 textarea 走默认行为）", () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    const evt = new Event("paste", { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { files: File[] };
    };
    evt.clipboardData = { files: [] };
    editor.dispatchEvent(evt);
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
    await new Promise((r) => setTimeout(r, 30));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "hi" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(queryAllByTestId(/^composer-attach-c-/)).toHaveLength(0);
  });

  it("v3 #50 · 附件 uploading 时 send · toast \"等附件传完再发\" + 不发", async () => {
    // 用 hanging uploadFile mock 让 chip 一直 uploading
    const api = createMockApi();
    api.uploadFile = (file): Promise<import("~/types/api").Upload> =>
      new Promise<import("~/types/api").Upload>(() => {
        // 不调 resolve, chip 永远 uploading 直到测试 end / abort
        void file;
      });
    setApiOverride(api);
    const onSubmit = vi.fn().mockResolvedValue(undefined);
    const { getByTestId, queryByTestId } = render(() => (
      <ApiProvider initialAuth="in">
        <MentionComposer candidates={[]} onSubmit={onSubmit} />
        <Toast />
      </ApiProvider>
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "hi" } });
    // chip 仍 uploading（hanging mock 没 resolve）
    fireEvent.click(getByTestId("mention-send"));
    expect(onSubmit).not.toHaveBeenCalled();
    // toast 同步渲染（pushToast 立即 setSignal）
    // toast 渲染 next microtask
    await Promise.resolve();
    await Promise.resolve();
    expect(queryByTestId("toast")?.textContent ?? "").toContain("等附件传完");
  });

  it("v3 #50 · uploading 中 ✕ 删 chip · 立即从 UI 下线（sync removeAttach）", () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const file = new File(["x"], "a.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [file], configurable: true });
    fireEvent.change(fileInput);
    const chips = queryAllByTestId(/^composer-attach-c-/);
    expect(chips).toHaveLength(1);
    expect(chips[0]?.getAttribute("data-status")).toBe("uploading");
    const cid = chips[0]?.getAttribute("data-testid")?.replace("composer-attach-", "") ?? "";
    // uploading 中 ✕ 删 → 同步从 UI 下线（abort 是异步副作用，本测试不验）
    fireEvent.click(getByTestId(`composer-attach-remove-${cid}`));
    expect(queryAllByTestId(/^composer-attach-c-/)).toHaveLength(0);
  });

  it("v3 #50 · 多文件并发上传（不串行）", async () => {
    const { getByTestId, queryAllByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const fileInput = getByTestId("composer-file-input") as HTMLInputElement;
    const a = new File(["a"], "a.txt", { type: "text/plain" });
    const b = new File(["b"], "b.txt", { type: "text/plain" });
    const c = new File(["c"], "c.txt", { type: "text/plain" });
    Object.defineProperty(fileInput, "files", { value: [a, b, c], configurable: true });
    fireEvent.change(fileInput);
    expect(queryAllByTestId(/^composer-attach-c-/)).toHaveLength(3);
    // 都立刻 uploading（说明并发了，没串行 await）
    const chips = queryAllByTestId(/^composer-attach-c-/);
    for (const ch of chips) expect(ch.getAttribute("data-status")).toBe("uploading");
    await new Promise((r) => setTimeout(r, 30));
    const after = queryAllByTestId(/^composer-attach-c-/);
    for (const ch of after) expect(ch.getAttribute("data-status")).toBe("done");
  });

  // ===== v3 #60 · dist node chip 路径 =====

  const NODE_HOME: MentionCandidate = {
    agent_id: "home",
    role: "node",
    role_display: "home",
    hint: "1/4 个任务",
    kind: "node",
  };
  const NODE_MAC: MentionCandidate = {
    agent_id: "mac-local",
    role: "node",
    role_display: "mac-local",
    hint: "0/8",
    kind: "node",
  };

  it("v3 #60 · 选 node 候选 → 节点 chip（蓝色 + testid 走 mention-chip-node-）", () => {
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN, NODE_MAC]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "用 @ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    expect(getByTestId("mention-chip-node-mac-local")).toBeTruthy();
    expect(queryByTestId("mention-chip-mac-local")).toBeNull();
    expect(getByTestId("mention-chip-node-mac-local").getAttribute("style")?.toLowerCase()).toContain("7aa0e5");
  });

  it("v3 #60 · 仅 node chip · 发送 · pinned_node=node_id · mentions 空 · target undefined", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = async (req: SerializedIntervene): Promise<void> => {
      captured = req;
    };
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[NODE_MAC]} onSubmit={onSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 跑测试" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(captured?.pinned_node).toBe("mac-local");
    expect(captured?.mentions).toEqual([]);
    expect(captured?.target).toBeUndefined();
    expect(captured?.text).toContain("跑测试");
  });

  it("v3 #60 · worker chip + node chip mix · 发送 target=worker · pinned_node=node", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = async (req: SerializedIntervene): Promise<void> => {
      captured = req;
    };
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[LUBAN, NODE_MAC]} onSubmit={onSubmit} />
    ));
    // 先 @鲁班
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@lu" } });
    fireEvent.click(getByTestId("mention-item-a-luban"));
    // 再 @mac-local
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " @ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 跑" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(captured?.target).toBe("a-luban");
    expect(captured?.mentions).toEqual(["a-luban"]);
    expect(captured?.pinned_node).toBe("mac-local");
    expect(captured?.multi).toBe(false);
    expect(captured?.multi_node).toBe(false);
  });

  it("v3 #60 · 两个 node chip · 发送时 toast \"多于一个节点\" 警示 · 取第一个", async () => {
    let captured = null as SerializedIntervene | null;
    const onSubmit = async (req: SerializedIntervene): Promise<void> => {
      captured = req;
    };
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <>
        <MentionComposer candidates={[NODE_HOME, NODE_MAC]} onSubmit={onSubmit} />
        <Toast />
      </>
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@ho" } });
    fireEvent.click(getByTestId("mention-item-node-home"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " @ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: " 跑" } });
    fireEvent.click(getByTestId("mention-send"));
    await new Promise((r) => setTimeout(r, 30));
    expect(captured?.pinned_node).toBe("home");
    expect(captured?.multi_node).toBe(true);
    expect(queryByTestId("toast")?.textContent ?? "").toContain("第一个");
  });

  it("v3 #60 · node chip ✕ 删 · chip 下线", () => {
    const { getByTestId, queryByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[NODE_MAC]} onSubmit={noopSubmit} />
    ));
    fireEvent.input(getByTestId("mention-editor"), { target: { value: "@ma" } });
    fireEvent.click(getByTestId("mention-item-node-mac-local"));
    expect(getByTestId("mention-chip-node-mac-local")).toBeTruthy();
    fireEvent.click(getByTestId("mention-chip-remove-node-mac-local"));
    expect(queryByTestId("mention-chip-node-mac-local")).toBeNull();
  });

  it("v3 #60 · placeholder 默认含 \"角色或节点\" 提示", () => {
    const { getByTestId } = renderWithApi(() => (
      <MentionComposer candidates={[]} onSubmit={noopSubmit} />
    ));
    const editor = getByTestId("mention-editor") as HTMLInputElement;
    expect(editor.placeholder).toContain("角色或节点");
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
