import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import {
  ApiProvider,
  setApiOverride,
  useApi,
  type MoreSubRoute,
  type NavRoute,
  type TabIndex,
} from "~/components/ApiProvider";
import { createMockApi } from "../mocks/api";
import { type Component, createSignal, onMount } from "solid-js";

afterEach(() => setApiOverride(null));

const Probe: Component<{
  onState: (s: { tab: TabIndex; sub: MoreSubRoute; nav: NavRoute }) => void;
}> = (props) => {
  const { activeTab, moreSub, navRoute } = useApi();
  const [_t, setT] = createSignal(0);
  onMount(() => {
    setT(1);
  });
  return (
    <div
      data-testid="probe"
      ref={() => {
        props.onState({ tab: activeTab(), sub: moreSub(), nav: navRoute() });
        void _t();
      }}
    />
  );
};

describe("ApiProvider · nav state (v1-session17 task #9 · 4 tab + 更多 hub)", () => {
  it("默认 activeTab=0（玄女）+ moreSub=null", () => {
    setApiOverride(createMockApi());
    let captured: { tab: TabIndex; sub: MoreSubRoute; nav: NavRoute } | null = null;
    const { unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Probe onState={(s) => (captured = s)} />
      </ApiProvider>
    ));
    expect(captured).not.toBeNull();
    expect(captured!.tab).toBe(0);
    expect(captured!.sub).toBeNull();
    expect(captured!.nav).toBeNull();
    unmount();
  });

  it("initialTab + initialMoreSub prop 钉死起始位置", () => {
    setApiOverride(createMockApi());
    let captured: { tab: TabIndex; sub: MoreSubRoute; nav: NavRoute } | null = null;
    const { unmount } = render(() => (
      <ApiProvider initialAuth="in" initialTab={3} initialMoreSub="memory">
        <Probe onState={(s) => (captured = s)} />
      </ApiProvider>
    ));
    expect(captured!.tab).toBe(3);
    expect(captured!.sub).toBe("memory");
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
        if (r.kind === "task") return r.task_id;
        if (r.kind === "worker") return r.agent_id;
        if (r.kind === "project") return r.project_id;
        return r.task_id;
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

  it("navPush 在非任务 tab 下且 sub 不允许时 noop（防御 misuse）", () => {
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
        if (r.kind === "task") return r.task_id;
        if (r.kind === "worker") return r.agent_id;
        if (r.kind === "project") return r.project_id;
        return r.task_id;
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

  it("navTo · deliverable 路由解析到 tab 3 + sub=deliverables", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { navTo, activeTab, moreSub, navRoute } = useApi();
      onMount(() => {
        navTo({ kind: "deliverable", project_id: "erp", task_id: "t-uuid" });
      });
      const navTask = (): string => {
        const r = navRoute();
        if (r && r.kind === "deliverable") return r.task_id;
        return "null";
      };
      return (
        <div>
          <span data-testid="tab-now">{String(activeTab())}</span>
          <span data-testid="sub-now">{String(moreSub())}</span>
          <span data-testid="nav-now">{navTask()}</span>
        </div>
      );
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("tab-now").textContent).toBe("3");
    expect(getByTestId("sub-now").textContent).toBe("deliverables");
    expect(getByTestId("nav-now").textContent).toBe("t-uuid");
    unmount();
  });

  it("navTo · project 路由解析到 tab 3 + sub=projects", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { navTo, activeTab, moreSub, navRoute } = useApi();
      onMount(() => {
        navTo({ kind: "project", project_id: "erp" });
      });
      const navProj = (): string => {
        const r = navRoute();
        if (r && r.kind === "project") return r.project_id;
        return "null";
      };
      return (
        <div>
          <span data-testid="tab-now">{String(activeTab())}</span>
          <span data-testid="sub-now">{String(moreSub())}</span>
          <span data-testid="nav-now">{navProj()}</span>
        </div>
      );
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("tab-now").textContent).toBe("3");
    expect(getByTestId("sub-now").textContent).toBe("projects");
    expect(getByTestId("nav-now").textContent).toBe("erp");
    unmount();
  });

  it("setMoreSub · 仅在 tab 3 下生效；其他 tab 静默 noop", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setActiveTab, setMoreSub, moreSub } = useApi();
      onMount(() => {
        setActiveTab(0);
        setMoreSub("memory"); // 不在 3 tab 下，应被 noop
      });
      return <span data-testid="sub-now">{String(moreSub())}</span>;
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("sub-now").textContent).toBe("null");
    unmount();
  });

  it("setMoreSub · tab 3 下生效；切 sub 自动清 navRoute", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setActiveTab, setMoreSub, moreSub, navPush, navRoute } = useApi();
      onMount(() => {
        setActiveTab(3);
        setMoreSub("projects");
        navPush({ kind: "project", project_id: "erp" });
        setMoreSub("memory"); // 切 sub → 清 navRoute
      });
      return (
        <div>
          <span data-testid="sub-now">{String(moreSub())}</span>
          <span data-testid="nav-now">{navRoute() === null ? "null" : "set"}</span>
        </div>
      );
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("sub-now").textContent).toBe("memory");
    expect(getByTestId("nav-now").textContent).toBe("null");
    unmount();
  });

  it("切 tab 离开「更多」时清 moreSub + navRoute", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setActiveTab, setMoreSub, navPush, moreSub, navRoute } = useApi();
      onMount(() => {
        setActiveTab(3);
        setMoreSub("projects");
        navPush({ kind: "project", project_id: "erp" });
        setActiveTab(0); // 离开更多
      });
      return (
        <div>
          <span data-testid="sub-now">{String(moreSub())}</span>
          <span data-testid="nav-now">{navRoute() === null ? "null" : "set"}</span>
        </div>
      );
    };
    const { getByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Wired />
      </ApiProvider>
    ));
    expect(getByTestId("sub-now").textContent).toBe("null");
    expect(getByTestId("nav-now").textContent).toBe("null");
    unmount();
  });
});
