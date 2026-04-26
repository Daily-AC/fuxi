import { describe, expect, it } from "vitest";
import {
  colorForTaskRole,
  formatDuration,
  formatTokens,
  shortTaskId,
} from "~/lib/format-task";

describe("formatDuration", () => {
  it("0 → 0:00", () => expect(formatDuration(0)).toBe("0:00"));
  it("12 秒 → 0:12", () => expect(formatDuration(12_000)).toBe("0:12"));
  it("3 分 20 秒 → 3:20", () => expect(formatDuration(200_000)).toBe("3:20"));
  it("超 1 小时 → H:MM:SS", () =>
    expect(formatDuration(3600_000 + 23 * 60_000 + 45_000)).toBe("1:23:45"));
  it("负数 / NaN → 0:00 兜底", () => {
    expect(formatDuration(-1)).toBe("0:00");
    expect(formatDuration(Number.NaN)).toBe("0:00");
  });
});

describe("formatTokens", () => {
  it("< 1k 原样", () => {
    expect(formatTokens(0)).toBe("");
    expect(formatTokens(123)).toBe("123");
    expect(formatTokens(999)).toBe("999");
  });
  it("1k–10k → x.yk", () => {
    expect(formatTokens(1234)).toBe("1.2k");
    expect(formatTokens(9876)).toBe("9.9k");
  });
  it("10k+ → 整 k", () => {
    expect(formatTokens(12_300)).toBe("12k");
    expect(formatTokens(123_400)).toBe("123k");
  });
  it("非法值 → 空", () => {
    expect(formatTokens(null)).toBe("");
    expect(formatTokens(undefined)).toBe("");
    expect(formatTokens(-1)).toBe("");
  });
});

describe("shortTaskId", () => {
  it("空 → #?", () => expect(shortTaskId("")).toBe("#?"));
  it("≤8 字符 → 全保留", () => expect(shortTaskId("abc")).toBe("#abc"));
  it(">8 字符 → 取尾 8", () =>
    expect(shortTaskId("uuid-1234567890abcdef")).toBe("#90abcdef"));
});

describe("colorForTaskRole", () => {
  it("玄女 → xuannv 紫", () => {
    expect(colorForTaskRole("xuannv")).toBe("#C4A8E8");
    expect(colorForTaskRole("玄女")).toBe("#C4A8E8");
  });
  it("鲁班 → luban 金", () => {
    expect(colorForTaskRole("luban")).toBe("#E5A547");
  });
  it("蒲松 → pusong 绿", () => {
    expect(colorForTaskRole("pusong")).toBe("#A0C277");
  });
  it("未知 → 默认次要色", () => {
    expect(colorForTaskRole("unknown")).toBe("#B8B0A0");
  });
});
