import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import type { Component } from "solid-js";
import { MascotProvider, useMascot } from "~/components/Mascot/MascotController";
import { Toast } from "~/components/Toast";
import { dismissToast, pushToast } from "~/lib/toast";

// Task 32 · 吉祥物事件接线单测。
// error 级 toast → 玄女惊讶（surprise）。用真 Toast 组件 + MascotProvider + 真 toast API，
// 不 mock dispatch——验证「显 error toast」的真实路径会让 mascot 状态机进 surprise。
//
// 探针组件：读 useMascot().mascotState().kind 渲染到 dom，方便断言。
const Probe: Component = () => {
  const { mascotState } = useMascot();
  return <span data-testid="probe-kind">{mascotState().kind}</span>;
};

describe("mascot wiring · error toast → surprise", () => {
  it("显 error toast 后 mascot 进 surprise", () => {
    dismissToast(); // 清掉前一个 case 残留的 toast
    const { getByTestId } = render(() => (
      <MascotProvider>
        <Toast />
        <Probe />
      </MascotProvider>
    ));

    // 初始 idle
    expect(getByTestId("probe-kind").textContent).toBe("idle");

    // 入队一条 error toast → Toast createEffect 派 {type:"error"} → surprise
    pushToast("门客正忙，等这轮跑完再发", "error");
    expect(getByTestId("probe-kind").textContent).toBe("surprise");

    dismissToast();
  });

  it("info 级 toast 不触发 surprise（仅 error 惊讶）", () => {
    dismissToast();
    const { getByTestId } = render(() => (
      <MascotProvider>
        <Toast />
        <Probe />
      </MascotProvider>
    ));

    expect(getByTestId("probe-kind").textContent).toBe("idle");
    pushToast("已派给项目 erp", "info");
    expect(getByTestId("probe-kind").textContent).toBe("idle");

    dismissToast();
  });
});
