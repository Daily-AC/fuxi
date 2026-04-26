import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { MentionChip } from "~/components/MentionChip";

describe("MentionChip (v3 #N2' / #37)", () => {
  it("渲染 @role_display + dot · 默认不可删", () => {
    const { getByTestId } = render(() => (
      <MentionChip agent_id="a-luban" role="luban" role_display="鲁班" />
    ));
    const chip = getByTestId("mention-chip-a-luban");
    expect(chip.textContent).toContain("@鲁班");
    expect(chip.getAttribute("data-role")).toBe("luban");
    expect(chip.querySelector("button")).toBeNull();
  });

  it("removable=true · 显 ✕ + onRemove 触发", () => {
    const onRemove = vi.fn();
    const { getByTestId } = render(() => (
      <MentionChip
        agent_id="a-pusong"
        role="pusong"
        role_display="蒲松"
        removable
        onRemove={onRemove}
      />
    ));
    const x = getByTestId("mention-chip-remove-a-pusong");
    fireEvent.click(x);
    expect(onRemove).toHaveBeenCalledOnce();
  });

  it("✕ aria-label 含角色名（屏幕阅读器友好）", () => {
    const { getByTestId } = render(() => (
      <MentionChip
        agent_id="a-luban"
        role="luban"
        role_display="鲁班"
        removable
        onRemove={() => undefined}
      />
    ));
    const x = getByTestId("mention-chip-remove-a-luban");
    expect(x.getAttribute("aria-label")).toContain("鲁班");
  });

  it("使用 colorForRole · 鲁班琥珀色（CSS var）", () => {
    const { getByTestId } = render(() => (
      <MentionChip agent_id="a-luban" role="luban" role_display="鲁班" />
    ));
    const chip = getByTestId("mention-chip-a-luban") as HTMLElement;
    // inline style 包含 chip-color css var
    expect(chip.getAttribute("style")).toContain("--chip-color");
    expect(chip.getAttribute("style")?.toLowerCase()).toContain("e5a547");
  });
});
