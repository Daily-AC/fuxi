import { describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { MascotLoader } from "~/components/ui/MascotLoader";
import { Skeleton } from "~/components/ui/Skeleton";
import { EmptyState } from "~/components/ui/EmptyState";
import { TypingDots } from "~/components/ui/TypingDots";

describe("MascotLoader", () => {
  it("renders mascot-loader containing a think-frame mascot", () => {
    const { getByTestId, unmount } = render(() => <MascotLoader />);
    expect(getByTestId("mascot-loader")).toBeTruthy();
    const img = getByTestId("mascot-img") as HTMLImageElement;
    expect(img.getAttribute("src")).toContain("xuannv-think");
    unmount();
  });

  it("renders the label text when given", () => {
    const { getByTestId, unmount } = render(() => <MascotLoader label="玄女正在思考" />);
    expect(getByTestId("mascot-loader").textContent).toContain("玄女正在思考");
    unmount();
  });
});

describe("Skeleton", () => {
  it("renders the skeleton testid", () => {
    const { getByTestId, unmount } = render(() => <Skeleton />);
    expect(getByTestId("skeleton")).toBeTruthy();
    unmount();
  });
});

describe("EmptyState", () => {
  it("renders empty-state with the title and a mascot", () => {
    const { getByTestId, unmount } = render(() => <EmptyState title="还没有任务" />);
    const el = getByTestId("empty-state");
    expect(el.textContent).toContain("还没有任务");
    expect(getByTestId("mascot-img")).toBeTruthy();
    unmount();
  });

  it("renders the hint text when given", () => {
    const { getByTestId, unmount } = render(() => (
      <EmptyState title="空空如也" hint="去发一条消息试试" />
    ));
    expect(getByTestId("empty-state").textContent).toContain("去发一条消息试试");
    unmount();
  });
});

describe("TypingDots", () => {
  it("renders the typing-dots testid", () => {
    const { getByTestId, unmount } = render(() => <TypingDots />);
    expect(getByTestId("typing-dots")).toBeTruthy();
    unmount();
  });
});
