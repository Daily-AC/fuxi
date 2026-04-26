import { describe, expect, it } from "vitest";
import {
  fromStoredMessage,
  mergeMessages,
  makeUserMessage,
  type Message,
} from "~/messages";
import type { StoredMessage } from "~/types/api";

describe("fromStoredMessage", () => {
  it("text role=user → UserMessage", () => {
    const s: StoredMessage = {
      id: "m1",
      conv_id: "xuannv",
      role: "user",
      kind: "text",
      content: "hi 玄女",
      ts: "2026-04-26T12:00:00Z",
    };
    const m = fromStoredMessage(s);
    expect(m).toMatchObject({ kind: "user", id: "m1", text: "hi 玄女", pending: false });
  });

  it("text role=xuannv → XuannvMessage non-streaming", () => {
    const s: StoredMessage = {
      id: "m2",
      conv_id: "xuannv",
      role: "xuannv",
      agent_id: "agent-uuid",
      kind: "text",
      content: "好的",
      ts: "2026-04-26T12:01:00Z",
    };
    const m = fromStoredMessage(s);
    expect(m).toMatchObject({
      kind: "xuannv",
      id: "m2",
      text: "好的",
      streaming: false,
      agent: "agent-uuid",
    });
  });

  it("Bug #24 · text 空 玄女条目过滤为 null（防空 bubble 残留）", () => {
    const s: StoredMessage = {
      id: "empty-xn",
      conv_id: "xuannv",
      role: "xuannv",
      agent_id: "x",
      kind: "text",
      content: "",
      ts: "2026-04-26T12:00:00Z",
    };
    expect(fromStoredMessage(s)).toBeNull();
  });

  it("Bug #24 · text 仅空白字符（空格 / \\n） 玄女条目也过滤", () => {
    const s: StoredMessage = {
      id: "ws-xn",
      conv_id: "xuannv",
      role: "xuannv",
      agent_id: "x",
      kind: "text",
      content: "   \n\t  ",
      ts: "2026-04-26T12:00:00Z",
    };
    expect(fromStoredMessage(s)).toBeNull();
  });

  it("Bug #24 · text 空 user 条目同样过滤（兜底）", () => {
    const s: StoredMessage = {
      id: "empty-u",
      conv_id: "xuannv",
      role: "user",
      kind: "text",
      content: "",
      ts: "2026-04-26T12:00:00Z",
    };
    expect(fromStoredMessage(s)).toBeNull();
  });

  it("file kind → FileMessage（attachments 占位 Upload）", () => {
    const s: StoredMessage = {
      id: "m3",
      conv_id: "xuannv",
      role: "user",
      kind: "file",
      content: { caption: "看图" },
      attachments: ["up-1", "up-2"],
      ts: "2026-04-26T12:02:00Z",
    };
    const m = fromStoredMessage(s);
    expect(m?.kind).toBe("file");
    if (m?.kind === "file") {
      expect(m.caption).toBe("看图");
      expect(m.attachments).toHaveLength(2);
      expect(m.attachments[0]?.id).toBe("up-1");
    }
  });

  it("阶段 4 才管的 kind · 返回 null", () => {
    const s: StoredMessage = {
      id: "m4",
      conv_id: "xuannv",
      role: "system",
      kind: "task_card",
      content: {},
      ts: "2026-04-26T12:03:00Z",
    };
    expect(fromStoredMessage(s)).toBeNull();
  });
});

describe("mergeMessages 去重 + 排序", () => {
  it("空增量 · 直接返回 prev", () => {
    const u = makeUserMessage("hi");
    const prev: Message[] = [u];
    expect(mergeMessages(prev, [])).toBe(prev);
  });

  it("相同 id · 不重复加", () => {
    const u = makeUserMessage("hi");
    const prev: Message[] = [u];
    const out = mergeMessages(prev, [u]);
    expect(out).toHaveLength(1);
  });

  it("按 ts 升序排（老的在前）", () => {
    const old: Message = {
      kind: "user",
      id: "old",
      text: "1",
      ts: 1000,
    };
    const newer: Message = {
      kind: "user",
      id: "new",
      text: "2",
      ts: 2000,
    };
    const out = mergeMessages([newer], [old]);
    expect(out[0]?.id).toBe("old");
    expect(out[1]?.id).toBe("new");
  });

  it("WS 推到的 id 跟历史里相同 · 不重复", () => {
    const histo: Message = {
      kind: "user",
      id: "shared-id",
      text: "from history",
      ts: 1000,
    };
    const live: Message = {
      kind: "user",
      id: "shared-id",
      text: "from ws (different text)",
      ts: 1500,
    };
    const out = mergeMessages([histo], [live]);
    expect(out).toHaveLength(1);
    expect(out[0]?.id).toBe("shared-id");
    // 保留先到的（历史）
    if (out[0]?.kind === "user") expect(out[0].text).toBe("from history");
  });
});
