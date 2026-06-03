import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { renderHook } from "@solidjs/testing-library";
import type { JSX } from "solid-js";
import { MascotProvider, useMascot } from "~/components/Mascot/MascotController";
import { QUIPS } from "~/components/Mascot/quips";

const wrap = (p: { children: JSX.Element }): JSX.Element => (
  <MascotProvider>{p.children}</MascotProvider>
);

describe("MascotController", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("hook 暴露 mascotState / dispatch / poke / quip", () => {
    const { result } = renderHook(useMascot, { wrapper: wrap });
    expect(typeof result.mascotState).toBe("function");
    expect(typeof result.dispatch).toBe("function");
    expect(typeof result.poke).toBe("function");
    expect(typeof result.quip).toBe("function");
    expect(result.mascotState().kind).toBe("idle");
  });

  it("task-done → happy，1.8s 后回 idle", () => {
    const { result } = renderHook(useMascot, { wrapper: wrap });
    result.dispatch({ type: "task-done" });
    expect(result.mascotState().kind).toBe("happy");
    vi.advanceTimersByTime(1800);
    expect(result.mascotState().kind).toBe("idle");
  });

  it("poke → poke + 非空 quip（出自 QUIPS），0.65s 后回 idle", () => {
    const { result } = renderHook(useMascot, { wrapper: wrap });
    result.poke();
    expect(result.mascotState().kind).toBe("poke");
    const q = result.quip() ?? "";
    expect(q.length).toBeGreaterThan(0);
    expect(QUIPS).toContain(q);
    vi.advanceTimersByTime(650);
    expect(result.mascotState().kind).toBe("idle");
  });

  it("error → surprise，1.2s 后回 idle", () => {
    const { result } = renderHook(useMascot, { wrapper: wrap });
    result.dispatch({ type: "error" });
    expect(result.mascotState().kind).toBe("surprise");
    vi.advanceTimersByTime(1200);
    expect(result.mascotState().kind).toBe("idle");
  });

  it("greet → wave，1.0s 后回 idle", () => {
    const { result } = renderHook(useMascot, { wrapper: wrap });
    result.dispatch({ type: "greet" });
    expect(result.mascotState().kind).toBe("wave");
    vi.advanceTimersByTime(1000);
    expect(result.mascotState().kind).toBe("idle");
  });

  it("idle 持续 30s 后睡眠（idle-timeout → sleep）", () => {
    const { result } = renderHook(useMascot, { wrapper: wrap });
    expect(result.mascotState().kind).toBe("idle");
    vi.advanceTimersByTime(30000);
    expect(result.mascotState().kind).toBe("sleep");
  });

  it("useMascot 在 provider 外使用抛错", () => {
    expect(() => renderHook(useMascot)).toThrow();
  });
});
