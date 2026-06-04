import { describe, expect, it } from "vitest";
import { shouldReloadOnControllerChange } from "~/lib/sw-update";

describe("sw-update · shouldReloadOnControllerChange", () => {
  it("既有 controller 被新 SW 顶替（更新）→ reload", () => {
    expect(shouldReloadOnControllerChange(true, false)).toBe(true);
  });

  it("首次安装（启动时无 controller）→ 不 reload（页面本就是最新，别无谓闪）", () => {
    expect(shouldReloadOnControllerChange(false, false)).toBe(false);
  });

  it("已在刷新中 → 不再 reload（防 reload 循环）", () => {
    expect(shouldReloadOnControllerChange(true, true)).toBe(false);
    expect(shouldReloadOnControllerChange(false, true)).toBe(false);
  });
});
