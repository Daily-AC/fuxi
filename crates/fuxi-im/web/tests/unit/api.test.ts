import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, createHttpClient } from "~/lib/api";

const realFetch = globalThis.fetch;

describe("createHttpClient", () => {
  beforeEach(() => {
    globalThis.fetch = vi.fn() as unknown as typeof fetch;
  });
  afterEach(() => {
    globalThis.fetch = realFetch;
  });

  it("fetchTasks(true) 拼 ?root=1", async () => {
    (globalThis.fetch as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response(JSON.stringify({ tasks: [] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    const c = createHttpClient();
    await c.fetchTasks(true);
    const call = (globalThis.fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(call?.[0]).toBe("/api/tasks?root=1");
  });

  it("非 2xx 抛 ApiError", async () => {
    (globalThis.fetch as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response("nope", { status: 500 }),
    );
    const c = createHttpClient();
    await expect(c.fetchTasks(false)).rejects.toBeInstanceOf(ApiError);
  });

  it("intervene POST 带 JSON body", async () => {
    (globalThis.fetch as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
      new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    const c = createHttpClient();
    await c.intervene({ text: "hi" });
    const call = (globalThis.fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(call?.[0]).toBe("/api/intervene");
    expect((call?.[1] as RequestInit | undefined)?.method).toBe("POST");
    expect((call?.[1] as RequestInit | undefined)?.body).toBe(JSON.stringify({ text: "hi" }));
  });
});
