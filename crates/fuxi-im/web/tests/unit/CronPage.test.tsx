import { afterEach, describe, expect, it } from "vitest";
import { render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { CronPage } from "~/views/pages/CronPage";
import { createMockApi } from "../mocks/api";

afterEach(() => setApiOverride(null));

describe("CronPage", () => {
  it("空状态文案", async () => {
    setApiOverride(createMockApi({ cron: { triggers: [] } }));
    const { findByText, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <CronPage />
      </ApiProvider>
    ));
    await findByText(/更漏空/);
    unmount();
  });

  it("非空 · 渲染 cron / webhook + disabled 卡置灰", async () => {
    setApiOverride(
      createMockApi({
        cron: {
          triggers: [
            {
              id: "trg-1",
              kind: "cron",
              enabled: true,
              intent: "晨报",
              summary: "0 9 * * *",
              tz: "Asia/Shanghai",
              consecutive_failures: 0,
              max_failures: 5,
              last_fired_at: null,
              created_at: "2026-05-01T00:00:00Z",
            },
            {
              id: "trg-2",
              kind: "webhook",
              enabled: false,
              intent: "失活 webhook",
              summary: "POST /hook/<id>",
              tz: null,
              consecutive_failures: 5,
              max_failures: 5,
              last_fired_at: "2026-05-06T00:00:00Z",
              created_at: "2026-05-01T00:00:00Z",
            },
          ],
        },
      }),
    );
    const { findByTestId, unmount } = render(() => (
      <ApiProvider initialAuth="in">
        <CronPage />
      </ApiProvider>
    ));
    const cron = await findByTestId("cron-entry-trg-1");
    expect(cron.textContent).toContain("晨报");
    expect(cron.textContent).toContain("0 9 * * *");
    expect(cron.textContent).toContain("Asia/Shanghai");
    const wh = await findByTestId("cron-entry-trg-2");
    expect(wh.textContent).toContain("失活 webhook");
    expect(wh.textContent).toContain("已停");
    expect(wh.textContent).toContain("连失败 5/5");
    unmount();
  });
});
