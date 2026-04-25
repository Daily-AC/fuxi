import { describe, expect, it } from "vitest";
import { relativeTime, shortAgentId, statusLabel } from "~/lib/format";

describe("format helpers", () => {
  describe("shortAgentId", () => {
    it("≤8 chars 原样返回", () => {
      expect(shortAgentId("abc")).toBe("abc");
      expect(shortAgentId("12345678")).toBe("12345678");
    });

    it(">8 chars 取头尾各 4 位用横杠", () => {
      expect(shortAgentId("abcdefghijklmnop")).toBe("abcd-mnop");
    });
  });

  describe("relativeTime", () => {
    const NOW = new Date("2026-04-26T12:00:00Z");

    it("分钟级", () => {
      expect(relativeTime("2026-04-26T11:55:00Z", NOW)).toBe("5 分钟前");
    });

    it("小时级", () => {
      expect(relativeTime("2026-04-26T09:00:00Z", NOW)).toBe("3 小时前");
    });

    it("天级", () => {
      expect(relativeTime("2026-04-24T12:00:00Z", NOW)).toBe("2 天前");
    });

    it("非法时间返回原字符串", () => {
      expect(relativeTime("not-a-date", NOW)).toBe("not-a-date");
    });
  });

  describe("statusLabel", () => {
    it("已知状态翻译", () => {
      expect(statusLabel("running")).toBe("进行中");
      expect(statusLabel("done")).toBe("完成");
      expect(statusLabel("failed")).toBe("失败");
      expect(statusLabel("blocked")).toBe("阻塞");
      expect(statusLabel("pending")).toBe("待派");
    });

    it("未知状态原样返回", () => {
      expect(statusLabel("zomg")).toBe("zomg");
    });
  });
});
