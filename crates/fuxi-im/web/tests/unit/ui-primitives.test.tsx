import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { Card } from "~/components/ui/Card";
import { ScreenHeader } from "~/components/ui/ScreenHeader";
import { SectionLabel } from "~/components/ui/SectionLabel";
import { StatePill } from "~/components/ui/StatePill";

describe("Card", () => {
  it("renders ui-card testid and its children", () => {
    const { getByTestId } = render(() => <Card>奶油糖果</Card>);
    const el = getByTestId("ui-card");
    expect(el.textContent).toContain("奶油糖果");
  });

  it("applies the global u-card class", () => {
    const { getByTestId } = render(() => <Card>x</Card>);
    expect(getByTestId("ui-card").className).toContain("u-card");
  });

  it("reflects the tone via data-tone", () => {
    const { getByTestId } = render(() => <Card tone="peach">x</Card>);
    expect(getByTestId("ui-card").getAttribute("data-tone")).toBe("peach");
  });
});

describe("ScreenHeader", () => {
  it("renders the title text", () => {
    const { getByTestId } = render(() => <ScreenHeader title="任务" />);
    expect(getByTestId("screen-title").textContent).toBe("任务");
  });

  it("renders no back button when onBack is absent", () => {
    const { queryByTestId } = render(() => <ScreenHeader title="任务" />);
    expect(queryByTestId("screen-back")).toBeNull();
  });

  it("calls onBack when the back button is clicked", () => {
    const onBack = vi.fn();
    const { getByTestId } = render(() => (
      <ScreenHeader title="任务" onBack={onBack} />
    ));
    fireEvent.click(getByTestId("screen-back"));
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("renders the trailing right slot", () => {
    const { getByTestId } = render(() => (
      <ScreenHeader title="任务" right={<span data-testid="hdr-right">右</span>} />
    ));
    expect(getByTestId("hdr-right").textContent).toBe("右");
  });
});

describe("SectionLabel", () => {
  it("renders section-label with its text", () => {
    const { getByTestId } = render(() => <SectionLabel>进行中</SectionLabel>);
    expect(getByTestId("section-label").textContent).toBe("进行中");
  });
});

describe("StatePill", () => {
  it("renders label text", () => {
    const { getByTestId } = render(() => <StatePill label="运行中" tone="running" />);
    expect(getByTestId("state-pill").textContent).toBe("运行中");
  });

  it("reflects the tone via data-tone", () => {
    const { getByTestId } = render(() => <StatePill label="完成" tone="done" />);
    expect(getByTestId("state-pill").getAttribute("data-tone")).toBe("done");
  });

  it("defaults to neutral tone", () => {
    const { getByTestId } = render(() => <StatePill label="未知" />);
    expect(getByTestId("state-pill").getAttribute("data-tone")).toBe("neutral");
  });
});
