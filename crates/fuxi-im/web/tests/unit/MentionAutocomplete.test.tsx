import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { MentionAutocomplete } from "~/components/MentionAutocomplete";
import type { MentionCandidate } from "~/lib/mentions";

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

describe("MentionAutocomplete (v3 #N2' / #37)", () => {
  it("visible=false · 不渲染", () => {
    const { queryByTestId } = render(() => (
      <MentionAutocomplete
        visible={false}
        candidates={[LUBAN]}
        highlightedIndex={0}
        onSelect={() => undefined}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    expect(queryByTestId("mention-popup")).toBeNull();
  });

  it("visible=true 渲染候选 + active 高亮 + role_display + hint", () => {
    const { getByTestId } = render(() => (
      <MentionAutocomplete
        visible
        candidates={[LUBAN, PUSONG]}
        highlightedIndex={0}
        onSelect={() => undefined}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    const luban = getByTestId("mention-item-a-luban");
    const pusong = getByTestId("mention-item-a-pusong");
    expect(luban.getAttribute("aria-selected")).toBe("true");
    expect(pusong.getAttribute("aria-selected")).toBe("false");
    expect(luban.textContent).toContain("鲁班");
    expect(luban.textContent).toContain("grep server");
  });

  it("候选空 · 显 \"没找到匹配的门客\"", () => {
    const { getByTestId } = render(() => (
      <MentionAutocomplete
        visible
        candidates={[]}
        highlightedIndex={0}
        onSelect={() => undefined}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    expect(getByTestId("mention-popup-empty").textContent).toContain("没找到");
  });

  it("tap 候选 · onSelect(候选)", () => {
    const onSelect = vi.fn();
    const { getByTestId } = render(() => (
      <MentionAutocomplete
        visible
        candidates={[LUBAN, PUSONG]}
        highlightedIndex={0}
        onSelect={onSelect}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    fireEvent.click(getByTestId("mention-item-a-pusong"));
    expect(onSelect).toHaveBeenCalledWith(PUSONG);
  });

  it("ArrowDown / ArrowUp · onMoveSelection(±1)", () => {
    const onMove = vi.fn();
    render(() => (
      <MentionAutocomplete
        visible
        candidates={[LUBAN, PUSONG]}
        highlightedIndex={0}
        onSelect={() => undefined}
        onCancel={() => undefined}
        onMoveSelection={onMove}
      />
    ));
    fireEvent.keyDown(document, { key: "ArrowDown" });
    expect(onMove).toHaveBeenCalledWith(1);
    fireEvent.keyDown(document, { key: "ArrowUp" });
    expect(onMove).toHaveBeenCalledWith(-1);
  });

  it("Enter · 选中高亮项", () => {
    const onSelect = vi.fn();
    render(() => (
      <MentionAutocomplete
        visible
        candidates={[LUBAN, PUSONG]}
        highlightedIndex={1}
        onSelect={onSelect}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    fireEvent.keyDown(document, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(PUSONG);
  });

  it("Esc · onCancel", () => {
    const onCancel = vi.fn();
    render(() => (
      <MentionAutocomplete
        visible
        candidates={[LUBAN]}
        highlightedIndex={0}
        onSelect={() => undefined}
        onCancel={onCancel}
        onMoveSelection={() => undefined}
      />
    ));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("visible=false 时键盘事件不触发回调", () => {
    const onMove = vi.fn();
    const onCancel = vi.fn();
    render(() => (
      <MentionAutocomplete
        visible={false}
        candidates={[LUBAN]}
        highlightedIndex={0}
        onSelect={() => undefined}
        onCancel={onCancel}
        onMoveSelection={onMove}
      />
    ));
    fireEvent.keyDown(document, { key: "ArrowDown" });
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onMove).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("候选 0 时按 Enter · noop（不调 onSelect）", () => {
    const onSelect = vi.fn();
    render(() => (
      <MentionAutocomplete
        visible
        candidates={[]}
        highlightedIndex={0}
        onSelect={onSelect}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    fireEvent.keyDown(document, { key: "Enter" });
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("highlightedIndex reactive 切换 · aria-selected 更新", () => {
    const [hi, setHi] = createSignal(0);
    const { getByTestId } = render(() => (
      <MentionAutocomplete
        visible
        candidates={[LUBAN, PUSONG]}
        highlightedIndex={hi()}
        onSelect={() => undefined}
        onCancel={() => undefined}
        onMoveSelection={() => undefined}
      />
    ));
    expect(getByTestId("mention-item-a-luban").getAttribute("aria-selected")).toBe("true");
    setHi(1);
    expect(getByTestId("mention-item-a-pusong").getAttribute("aria-selected")).toBe("true");
    expect(getByTestId("mention-item-a-luban").getAttribute("aria-selected")).toBe("false");
  });
});
