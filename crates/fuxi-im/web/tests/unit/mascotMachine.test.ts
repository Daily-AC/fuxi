import { describe, expect, it } from "vitest";
import { reduceMascot, type MascotState, type MascotEvent } from "~/components/Mascot/mascotMachine";

describe("mascotMachine", () => {
  const idle: MascotState = { kind: "idle" };
  // 类型完整性：确认 MascotEvent union 可被赋值
  const _typeCheck: MascotEvent = { type: "settle" };
  void _typeCheck;
  it("流式开始 → talk", () => {
    expect(reduceMascot(idle, { type: "stream-start" }).kind).toBe("talk");
  });
  it("门客在跑 → think", () => {
    expect(reduceMascot(idle, { type: "work-running" }).kind).toBe("think");
  });
  it("任务完成 → happy", () => {
    expect(reduceMascot(idle, { type: "task-done" }).kind).toBe("happy");
  });
  it("报错 → surprise", () => {
    expect(reduceMascot(idle, { type: "error" }).kind).toBe("surprise");
  });
  it("戳一下 → poke", () => {
    expect(reduceMascot(idle, { type: "poke" }).kind).toBe("poke");
  });
  it("happy/surprise/poke 是瞬时态，settle 后回 idle", () => {
    expect(reduceMascot({ kind: "happy" }, { type: "settle" }).kind).toBe("idle");
    expect(reduceMascot({ kind: "poke" }, { type: "settle" }).kind).toBe("idle");
  });
  it("idle 计时到点 → sleep；任何输入唤醒回 idle", () => {
    expect(reduceMascot(idle, { type: "idle-timeout" }).kind).toBe("sleep");
    expect(reduceMascot({ kind: "sleep" }, { type: "poke" }).kind).toBe("poke");
  });
  it("talk 期间 error 优先级更高 → surprise", () => {
    expect(reduceMascot({ kind: "talk" }, { type: "error" }).kind).toBe("surprise");
  });
});
