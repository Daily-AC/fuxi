import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { BottomTabBar, type TabSpec, type TabIndex } from "~/components/BottomTabBar";

const TABS: TabSpec[] = [
  { key: "xuannv", label: "玄女" },
  { key: "tasks", label: "任务" },
  { key: "nodes", label: "节点" },
];

describe("BottomTabBar (v3 #N1' / #36)", () => {
  it("渲染 3 项 + active=0 高亮玄女", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    const x = getByTestId("tab-xuannv");
    const t = getByTestId("tab-tasks");
    const n = getByTestId("tab-nodes");
    expect(x.getAttribute("aria-selected")).toBe("true");
    expect(t.getAttribute("aria-selected")).toBe("false");
    expect(n.getAttribute("aria-selected")).toBe("false");
  });

  it("tap tab-tasks · onChange(1)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-tasks"));
    expect(onChange).toHaveBeenCalledWith(1);
  });

  it("tap tab-nodes · onChange(2)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-nodes"));
    expect(onChange).toHaveBeenCalledWith(2);
  });

  it("active 切换 reactive · signal driven", () => {
    const onChange = vi.fn();
    const [active, setActive] = createSignal<TabIndex>(0);
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={active()} onChange={onChange} />
    ));
    expect(getByTestId("tab-xuannv").getAttribute("aria-selected")).toBe("true");
    setActive(1);
    expect(getByTestId("tab-tasks").getAttribute("aria-selected")).toBe("true");
    expect(getByTestId("tab-xuannv").getAttribute("aria-selected")).toBe("false");
  });

  it("aria-label 用 label 文本（屏幕阅读器友好）", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    expect(getByTestId("tab-xuannv").getAttribute("aria-label")).toBe("玄女");
    expect(getByTestId("tab-tasks").getAttribute("aria-label")).toBe("任务");
    expect(getByTestId("tab-nodes").getAttribute("aria-label")).toBe("节点");
  });
});
