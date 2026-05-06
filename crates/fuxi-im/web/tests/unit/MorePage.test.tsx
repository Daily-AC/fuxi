import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride, useApi } from "~/components/ApiProvider";
import { MorePage } from "~/views/pages/MorePage";
import { createMockApi } from "../mocks/api";
import { type Component } from "solid-js";

afterEach(() => setApiOverride(null));

const SubProbe: Component = () => {
  const { moreSub } = useApi();
  return <span data-testid="sub-now">{String(moreSub())}</span>;
};

describe("MorePage · 「更多」hub tile grid", () => {
  it("渲染 8 张 tile（节点 / 项目 / 工作者 / 交付物 / 记忆 / 角色 / 更漏 / 设置）", () => {
    setApiOverride(createMockApi());
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3}>
        <MorePage />
      </ApiProvider>
    ));
    for (const sub of [
      "nodes",
      "projects",
      "workers",
      "deliverables",
      "memory",
      "roles",
      "cron",
      "settings",
    ]) {
      expect(getByTestId(`more-tile-${sub}`)).toBeTruthy();
    }
    unmount();
  });

  it("tap tile-memory · setMoreSub('memory')", () => {
    setApiOverride(createMockApi());
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3}>
        <MorePage />
        <SubProbe />
      </ApiProvider>
    ));
    fireEvent.click(getByTestId("more-tile-memory"));
    expect(getByTestId("sub-now").textContent).toBe("memory");
    unmount();
  });

  it("tap tile-roles · setMoreSub('roles')", () => {
    setApiOverride(createMockApi());
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3}>
        <MorePage />
        <SubProbe />
      </ApiProvider>
    ));
    fireEvent.click(getByTestId("more-tile-roles"));
    expect(getByTestId("sub-now").textContent).toBe("roles");
    unmount();
  });

  it("tap tile-cron · setMoreSub('cron')", () => {
    setApiOverride(createMockApi());
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3}>
        <MorePage />
        <SubProbe />
      </ApiProvider>
    ));
    fireEvent.click(getByTestId("more-tile-cron"));
    expect(getByTestId("sub-now").textContent).toBe("cron");
    unmount();
  });
});
