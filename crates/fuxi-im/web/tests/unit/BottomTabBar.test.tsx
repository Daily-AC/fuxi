import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { BottomTabBar, type TabSpec, type TabIndex } from "~/components/BottomTabBar";

// daimeng 奶油糖果重构 · 4 tab [家][聊天][任务][更多] + SVG 图标
const TABS: TabSpec[] = [
  { key: "home", label: "家" },
  { key: "xuannv", label: "聊天" },
  { key: "tasks", label: "任务" },
  { key: "more", label: "更多" },
];

describe("BottomTabBar (daimeng · 4 tab 家/聊天/任务/更多)", () => {
  it("渲染 4 项 + active=0 高亮家", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    const h = getByTestId("tab-home");
    const x = getByTestId("tab-xuannv");
    const t = getByTestId("tab-tasks");
    const m = getByTestId("tab-more");
    expect(h.getAttribute("aria-selected")).toBe("true");
    expect(x.getAttribute("aria-selected")).toBe("false");
    expect(t.getAttribute("aria-selected")).toBe("false");
    expect(m.getAttribute("aria-selected")).toBe("false");
  });

  it("tab 图标无 emoji：家=小玄女头像 img，其余 inline SVG", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    // 家 tab 改用小玄女头像（webp avatar），更显眼且绑产品主体；仍非 emoji。
    const home = getByTestId("tab-home-icon");
    expect(home.tagName.toLowerCase()).toBe("img");
    expect(home.getAttribute("src")).toContain("/mascot/xuannv-");
    expect(getByTestId("tab-xuannv-icon").tagName.toLowerCase()).toBe("svg");
    expect(getByTestId("tab-tasks-icon").tagName.toLowerCase()).toBe("svg");
    expect(getByTestId("tab-more-icon").tagName.toLowerCase()).toBe("svg");
  });

  it("tap tab-xuannv · onChange(1)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-xuannv"));
    expect(onChange).toHaveBeenCalledWith(1);
  });

  it("tap tab-tasks · onChange(2)", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("tab-tasks"));
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
    expect(getByTestId("tab-home").getAttribute("aria-selected")).toBe("true");
    setActive(3);
    expect(getByTestId("tab-more").getAttribute("aria-selected")).toBe("true");
    expect(getByTestId("tab-home").getAttribute("aria-selected")).toBe("false");
  });

  it("aria-label 用 label 文本（屏幕阅读器友好）", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <BottomTabBar tabs={TABS} active={0} onChange={onChange} />
    ));
    expect(getByTestId("tab-home").getAttribute("aria-label")).toBe("家");
    expect(getByTestId("tab-xuannv").getAttribute("aria-label")).toBe("聊天");
    expect(getByTestId("tab-tasks").getAttribute("aria-label")).toBe("任务");
    expect(getByTestId("tab-more").getAttribute("aria-label")).toBe("更多");
  });

  it("badge 显示 · unread > 0 时显数字", () => {
    const onChange = vi.fn();
    const tabs: TabSpec[] = TABS.map((t) =>
      t.key === "home" ? { ...t, badge: 5 } : t,
    );
    const { getByTestId, queryByTestId } = render(() => (
      <BottomTabBar tabs={tabs} active={0} onChange={onChange} />
    ));
    expect(getByTestId("tab-home-badge").textContent).toBe("5");
    expect(queryByTestId("tab-more-badge")).toBeNull();
  });
});
