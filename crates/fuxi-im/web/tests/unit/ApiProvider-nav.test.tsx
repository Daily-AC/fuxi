import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import {
  ApiProvider,
  setApiOverride,
  useApi,
  type NavRoute,
  type PageIndex,
} from "~/components/ApiProvider";
import { createMockApi } from "../mocks/api";
import { type Component, createSignal, onMount } from "solid-js";

afterEach(() => setApiOverride(null));

const Probe: Component<{ onState: (s: { page: PageIndex; nav: NavRoute }) => void }> = (
  props,
) => {
  const { currentPage, navRoute } = useApi();
  const [_t, setT] = createSignal(0);
  onMount(() => {
    setT(1);
  });
  // 用 effect 让外部观察
  return (
    <div
      data-testid="probe"
      ref={() => {
        // 每次 currentPage / navRoute 变化时通知
        // 简化：mount 后立即报一次（外部测试也可手动操作 setActiveSheet 后查 props.onState）
        props.onState({ page: currentPage(), nav: navRoute() });
        void _t();
      }}
    />
  );
};

describe("ApiProvider · nav state", () => {
  it("默认 currentPage=1（玄女）", () => {
    setApiOverride(createMockApi());
    let captured: { page: PageIndex; nav: NavRoute } | null = null;
    const { unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <Probe onState={(s) => (captured = s)} />
      </ApiProvider>
    ));
    expect(captured).not.toBeNull();
    expect(captured!.page).toBe(1);
    expect(captured!.nav).toBeNull();
    unmount();
  });

  it("initialPage prop 钉死起始页", () => {
    setApiOverride(createMockApi());
    let captured: { page: PageIndex; nav: NavRoute } | null = null;
    const { unmount } = render(() => (
      <ApiProvider initialAuth="in" initialPage={2}>
        <Probe onState={(s) => (captured = s)} />
      </ApiProvider>
    ));
    expect(captured!.page).toBe(2);
    unmount();
  });

  it("setCurrentPage / navPush / navPop 互不打架", () => {
    setApiOverride(createMockApi());
    const Wired: Component = () => {
      const { setCurrentPage, navPush, navPop, currentPage, navRoute } = useApi();
      onMount(() => {
        setCurrentPage(2);
        navPush({ kind: "worker", agent_id: "abc-123", role_display: "鲁班" });
      });
      return (
        <div>
          <span data-testid="page-now">{String(currentPage())}</span>
          <span data-testid="nav-now">{navRoute()?.agent_id ?? "null"}</span>
          <button
            type="button"
            data-testid="pop"
            onClick={() => navPop()}
          >
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
    expect(getByTestId("page-now").textContent).toBe("2");
    expect(getByTestId("nav-now").textContent).toBe("abc-123");
    getByTestId("pop").click();
    expect(getByTestId("nav-now").textContent).toBe("null");
    expect(getByTestId("page-now").textContent).toBe("2"); // pop 不动 page
    unmount();
  });
});
