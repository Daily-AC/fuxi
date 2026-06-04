import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ListRow } from "~/components/ui/ListRow";
import { Tile } from "~/components/ui/Tile";
import { ToggleRow } from "~/components/ui/ToggleRow";

describe("ListRow", () => {
  it("renders list-row-title with title text", () => {
    const { getByTestId } = render(() => <ListRow title="任务一" />);
    expect(getByTestId("list-row-title").textContent).toBe("任务一");
  });

  it("calls onClick when the row is clicked", () => {
    const onClick = vi.fn();
    const { getByTestId } = render(() => (
      <ListRow title="任务一" onClick={onClick} />
    ));
    fireEvent.click(getByTestId("list-row"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("renders the right slot content", () => {
    const { getByTestId } = render(() => (
      <ListRow title="任务一" right={<span data-testid="row-right">右</span>} />
    ));
    expect(getByTestId("row-right").textContent).toBe("右");
  });

  it("renders the subtitle when given", () => {
    const { getByText } = render(() => (
      <ListRow title="任务一" subtitle="副标题" />
    ));
    expect(getByText("副标题")).toBeTruthy();
  });
});

describe("Tile", () => {
  it("renders tile-label text", () => {
    const { getByTestId } = render(() => <Tile label="节点" />);
    expect(getByTestId("tile-label").textContent).toBe("节点");
  });

  it("calls onClick when clicked", () => {
    const onClick = vi.fn();
    const { getByTestId } = render(() => (
      <Tile label="节点" onClick={onClick} />
    ));
    fireEvent.click(getByTestId("tile"));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("renders the desc when given", () => {
    const { getByText } = render(() => <Tile label="节点" desc="集群算力" />);
    expect(getByText("集群算力")).toBeTruthy();
  });
});

describe("ToggleRow", () => {
  it("renders the title text", () => {
    const { getByText } = render(() => (
      <ToggleRow title="通知" checked={false} onChange={() => {}} />
    ));
    expect(getByText("通知")).toBeTruthy();
  });

  it("reflects checked via aria-checked on the switch", () => {
    const { getByTestId } = render(() => (
      <ToggleRow title="通知" checked={true} onChange={() => {}} />
    ));
    expect(getByTestId("toggle-switch").getAttribute("aria-checked")).toBe(
      "true",
    );
  });

  it("reflects unchecked via aria-checked", () => {
    const { getByTestId } = render(() => (
      <ToggleRow title="通知" checked={false} onChange={() => {}} />
    ));
    expect(getByTestId("toggle-switch").getAttribute("aria-checked")).toBe(
      "false",
    );
  });

  it("calls onChange with the negated value when the switch is clicked", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <ToggleRow title="通知" checked={false} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("toggle-switch"));
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("toggles from checked to unchecked", () => {
    const onChange = vi.fn();
    const { getByTestId } = render(() => (
      <ToggleRow title="通知" checked={true} onChange={onChange} />
    ));
    fireEvent.click(getByTestId("toggle-switch"));
    expect(onChange).toHaveBeenCalledWith(false);
  });
});
