import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { MoreSubShell } from "~/components/MoreSubShell";
import { createMockApi } from "../mocks/api";
import { type Component } from "solid-js";

afterEach(() => setApiOverride(null));

const Probe: Component = () => {
  const { moreSub } = useApi();
  return <span data-testid="sub-now">{String(moreSub())}</span>;
};

describe("MoreSubShell", () => {
  it("渲染 title + 返回按钮 + children body", () => {
    setApiOverride(createMockApi());
    const { getByTestId, getByText, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={4} initialMoreSub="memory">
        <MoreSubShell title="记忆" testIdSuffix="memory">
          <p data-testid="payload">payload</p>
        </MoreSubShell>
      </ApiProvider>
    ));
    expect(getByText("记忆")).toBeTruthy();
    expect(getByTestId("more-sub-back")).toBeTruthy();
    expect(getByTestId("payload")).toBeTruthy();
    expect(getByTestId("more-sub-memory")).toBeTruthy();
    unmount();
  });

  it("点 ‹ 更多 · setMoreSub(null) 回 hub", () => {
    setApiOverride(createMockApi());
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={4} initialMoreSub="cron">
        <MoreSubShell title="更漏" testIdSuffix="cron">
          <span>x</span>
        </MoreSubShell>
        <Probe />
      </ApiProvider>
    ));
    expect(getByTestId("sub-now").textContent).toBe("cron");
    fireEvent.click(getByTestId("more-sub-back"));
    expect(getByTestId("sub-now").textContent).toBe("null");
    unmount();
  });

  it("hideTitle=true · 不显 shell 标题（子页自带 header 时）", () => {
    setApiOverride(createMockApi());
    const { queryByText, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={4} initialMoreSub="nodes">
        <MoreSubShell title="节点" hideTitle testIdSuffix="nodes">
          <h1>节点</h1>
        </MoreSubShell>
      </ApiProvider>
    ));
    // 子页自己的 h1 渲染（query 应只找到一个 "节点"）
    const els = Array.from(document.querySelectorAll("h1")).filter(
      (el) => el.textContent === "节点",
    );
    expect(els.length).toBe(1);
    expect(queryByText("节点")).toBeTruthy();
    unmount();
  });
});
