import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import {
  ApiProvider,
  setApiOverride,
  useApi,
  type NavRoute,
  type TabIndex,
} from "~/components/ApiProvider";
import { createMockApi } from "../mocks/api";
import { type Component, createSignal, onMount } from "solid-js";

afterEach(() => setApiOverride(null));

const Probe: Component<{ onState: (s: { tab: TabIndex; nav: NavRoute }) => void }> = (
  props,
) => {
  const { activeTab, navRoute } = useApi();
  const [_t, setT] = createSignal(0);
  onMount(() => {
    setT(1);
  });
  return (
    <div
      data-testid="probe"
      ref={() => {
        props.onState({ tab: activeTab(), nav: navRoute() });
        void _t();
      }}
    />
  );
};

describe("ApiProvider · nav state (v3)", () => {
  it("默认 activeTab=0（玄女）", () => {
    setApiOverride(createMockApi());
    let captured: { tab: TabIndex; nav: NavRoute } | null = null;
    const { unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Probe onState={(s) => (captured = s)} />
      </ApiProvider>
    ));
    expect(captured).not.toBeNull();
    expect(captured!.tab).toBe(0);
    expect(captured!.nav).toBeNull();
    unmount();
  });

  it("initialTab prop 钉死起始 tab", () => {
    setApiOverride(createMockApi());
    let captured: { tab: TabIndex; nav: NavRoute } | null = null;
    const { unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={1}>
        <Probe onState={(s) => (captured = s)} />
      </ApiProvider>
    ));
    expect(captured!.tab).toBe(1);
    unmount();
  });

  it("setActiveTab + 任务 tab 下 navPush · 闭环", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setActiveTab, navPush, navPop, activeTab, navRoute } = useApi();
      onMount(() => {
        setActiveTab(1); // 任务 tab
        navPush({ kind: "task", task_id: "t-uuid", title: "查 ERP" });
      });
      const navId = (): string => {
        const r = navRoute();
        if (!r) return "null";
        return r.kind === "task" ? r.task_id : r.agent_id;
      };
      return (
        <div>
          <span data-testid="tab-now">{String(activeTab())}</span>
          <span data-testid="nav-now">{navId()}</span>
          <button type="button" data-testid="pop" onClick={() => navPop()}>
            pop
          </button>
        </div>
      );
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("tab-now").textContent).toBe("1");
    expect(getByTestId("nav-now").textContent).toBe("t-uuid");
    getByTestId("pop").click();
    expect(getByTestId("nav-now").textContent).toBe("null");
    expect(getByTestId("tab-now").textContent).toBe("1"); // pop 不动 tab
    unmount();
  });

  it("navPush 在非任务 tab 下 noop（防御 misuse）", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setActiveTab, navPush, navRoute } = useApi();
      onMount(() => {
        setActiveTab(0); // 玄女 tab
        navPush({ kind: "task", task_id: "should-not-push" });
      });
      const id = (): string => {
        const r = navRoute();
        if (!r) return "null";
        return r.kind === "task" ? r.task_id : r.agent_id;
      };
      return <span data-testid="nav-now">{id()}</span>;
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("nav-now").textContent).toBe("null");
    unmount();
  });

  it("切 tab 自动清 navRoute（避免跨 tab 残留二层 push）", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setActiveTab, navPush, navRoute } = useApi();
      onMount(() => {
        setActiveTab(1);
        navPush({ kind: "task", task_id: "t-1" });
        setActiveTab(0); // 切到玄女 tab → 应清 nav
      });
      const id = (): string => {
        const r = navRoute();
        if (!r) return "null";
        return r.kind === "task" ? r.task_id : r.agent_id;
      };
      return <span data-testid="nav-now">{id()}</span>;
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("nav-now").textContent).toBe("null");
    unmount();
  });
});
