import { describe, expect, it } from "vitest";
import { tokens, colorForRole } from "~/tokens";

describe("奶油糖果 tokens", () => {
  it("背景是暖奶白不是旧暗棕", () => {
    expect(tokens.bg.toUpperCase()).toBe("#FFF8F0");
    expect(tokens.bg).not.toBe("#1F1E1B");
  });
  it("主 accent 是蜜桃", () => {
    expect(tokens.accent.toUpperCase()).toBe("#FFB877");
  });
  it("玄女角色色是 lavender", () => {
    expect(colorForRole("xuannv").toUpperCase()).toBe("#C9B6E8");
    expect(colorForRole("玄女")).toBe(colorForRole("xuannv"));
  });
  it("卡片圆角更胖（≥20）", () => {
    expect(tokens.radius.card).toBeGreaterThanOrEqual(20);
  });
});
