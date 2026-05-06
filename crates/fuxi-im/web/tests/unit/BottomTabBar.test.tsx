import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { BottomTabBar, type TabSpec, type TabIndex } from "~/components/BottomTabBar";

// v1-session17 task #9 起 4 tab + 「更多」hub
const TABS: TabSpec[] = [
  { key: "xuannv", label: "玄女" },
  { key: "tasks", label: "任务" },
  { key: "notifications", label: "通知" },
  { key: "more", label: "更多" },
];

describe("BottomTabBar (v1-session17 task #9 · 4 tab)", () => {
  it("渲染 4 项 + active=0 高亮玄女", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    const x = getByTestId("tab-xuannv");
    const t = getByTestId("tab-tasks");
    const n = getByTestId("tab-notifications");
    const m = getByTestId("tab-more");
    expect(x.getAttribute("aria-selected")).toBe("true");
    expect(t.getAttribute("aria-selected")).toBe("false");
    expect(n.getAttribute("aria-selected")).toBe("false");
    expect(m.getAttribute("aria-selected")).toBe("false");
  });

  it("tap tab-tasks · onChange(1)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-tasks"));
    expect(onChange).toHaveBeenCalledWith(1);
  });

  it("tap tab-notifications · onChange(2)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-notifications"));
    expect(onChange).toHaveBeenCalledWith(2);
  });

  it("tap tab-more · onChange(3)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-more"));
    expect(onChange).toHaveBeenCalledWith(3);
  });

  it("active 切换 reactive · signal driven", () => {
    const onChange = vi.fn();
    const [active, setActive] = createSignal<TabIndex>(0);
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={active()} onChange={onChange} />
    ));
    expect(getByTestId("tab-xuannv").getAttribute("aria-selected")).toBe("true");
    setActive(3);
    expect(getByTestId("tab-more").getAttribute("aria-selected")).toBe("true");
    expect(getByTestId("tab-xuannv").getAttribute("aria-selected")).toBe("false");
  });

  it("aria-label 用 label 文本（屏幕阅读器友好）", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    expect(getByTestId("tab-xuannv").getAttribute("aria-label")).toBe("玄女");
    expect(getByTestId("tab-tasks").getAttribute("aria-label")).toBe("任务");
    expect(getByTestId("tab-notifications").getAttribute("aria-label")).toBe("通知");
    expect(getByTestId("tab-more").getAttribute("aria-label")).toBe("更多");
  });

  it("badge 显示在通知 tab · unread > 0 时显数字", () => {
    const onChange = vi.fn();
    const tabs: TabSpec[] = TABS.map((t) =>
      t.key === "notifications" ? { ...t, badge: 5 } : t,
    );
    const { getByTestId, queryByTestId } = render(() => (
      <BottomTabBar tabs={tabs} active={0} onChange={onChange} />
    ));
    expect(getByTestId("tab-notifications-badge").textContent).toBe("5");
    expect(queryByTestId("tab-more-badge")).toBeNull();
  });
});
