import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { Header } from "~/components/Header";

describe("Header", () => {
  it("渲染三 tap target：任务 / 玄女 + 状态 / 节点", () => {
    const { getByTestId, unmount } = render(() => (
      <Header online onOpenTasks={() => {}} onOpenNodes={() => {}} />
    ));
    expect(getByTestId("header-tasks").textContent).toContain("任务");
    expect(getByTestId("header-nodes").textContent).toContain("节点");
    expect(getByTestId("header-center").textContent).toContain("玄女");
    expect(getByTestId("header-center").textContent).toContain("在线");
    unmount();
  });

  it("offline 状态显示 '重连中'", () => {
    const { getByTestId, unmount } = render(() => (
      <Header online={false} onOpenTasks={() => {}} onOpenNodes={() => {}} />
    ));
    expect(getByTestId("header-center").textContent).toContain("重连中");
    unmount();
  });

  it("点任务 / 节点触发对应回调", () => {
    const onTasks = vi.fn();
    const onNodes = vi.fn();
    const { getByTestId, unmount } = render(() => (
      <Header online onOpenTasks={onTasks} onOpenNodes={onNodes} />
    ));
    fireEvent.click(getByTestId("header-tasks"));
    fireEvent.click(getByTestId("header-nodes"));
    expect(onTasks).toHaveBeenCalledTimes(1);
    expect(onNodes).toHaveBeenCalledTimes(1);
    unmount();
  });
});
