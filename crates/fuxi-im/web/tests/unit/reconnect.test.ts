import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { startReconnectingSocket } from "~/lib/reconnect";

class FakeWS extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  readyState = 0;
  closed = false;
  url = "ws://test";
  open(): void {
    this.readyState = 1;
    this.dispatchEvent(new Event("open"));
  }
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.readyState = 3;
    this.dispatchEvent(new Event("close"));
  }
  emitMessage(data: string): void {
    this.dispatchEvent(new MessageEvent("message", { data }));
  }
}

describe("startReconnectingSocket", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("成功 open 后 onOpen 被调用，attempts=1", () => {
    const sockets: FakeWS[] = [];
    const onOpen = vi.fn();
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      { onOpen },
      { initialDelayMs: 100, jitter: 0 },
    );
    sockets[0]?.open();
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(ctrl.attempts()).toBe(1);
    ctrl.dispose();
  });

  it("close 后按指数退避重连：100→200→400→800→1600→cap 1000", () => {
    const sockets: FakeWS[] = [];
    const onClose = vi.fn();
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      { onClose },
      { initialDelayMs: 100, maxDelayMs: 1000, factor: 2, jitter: 0 },
    );
    // 第 1 次连接立即开
    expect(ctrl.attempts()).toBe(1);
    sockets[0]?.open();
    sockets[0]?.close();
    // 100ms 后第 2 次
    vi.advanceTimersByTime(100);
    expect(ctrl.attempts()).toBe(2);
    sockets[1]?.close();
    // 200ms 后第 3 次
    vi.advanceTimersByTime(200);
    expect(ctrl.attempts()).toBe(3);
    sockets[2]?.close();
    vi.advanceTimersByTime(400);
    expect(ctrl.attempts()).toBe(4);
    sockets[3]?.close();
    vi.advanceTimersByTime(800);
    expect(ctrl.attempts()).toBe(5);
    sockets[4]?.close();
    // 上限 1000 取代 1600
    vi.advanceTimersByTime(1000);
    expect(ctrl.attempts()).toBe(6);
    expect(onClose).toHaveBeenCalledTimes(5);
    ctrl.dispose();
  });

  it("dispose() 后即使触发 close 也不再重连（防风暴的核心断言）", () => {
    const sockets: FakeWS[] = [];
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      {},
      { initialDelayMs: 50, jitter: 0 },
    );
    sockets[0]?.open();
    ctrl.dispose();
    // 模拟服务端踢链接：close 事件后不应该再 open 新 socket
    sockets[0]?.close();
    vi.advanceTimersByTime(10_000);
    expect(ctrl.attempts()).toBe(1);
  });

  it("dispose() 显式关已开 socket（防 leak）", () => {
    const sockets: FakeWS[] = [];
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      {},
      { initialDelayMs: 50, jitter: 0 },
    );
    sockets[0]?.open();
    expect(sockets[0]?.closed).toBe(false);
    ctrl.dispose();
    expect(sockets[0]?.closed).toBe(true);
  });

  it("成功重连后 backoff 重置回 initial", () => {
    const sockets: FakeWS[] = [];
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      {},
      { initialDelayMs: 100, factor: 2, jitter: 0 },
    );
    sockets[0]?.open();
    sockets[0]?.close();
    vi.advanceTimersByTime(100);
    expect(ctrl.attempts()).toBe(2);
    sockets[1]?.close();
    vi.advanceTimersByTime(200);
    expect(ctrl.attempts()).toBe(3);
    // 这次成功 → backoff 应被重置
    sockets[2]?.open();
    sockets[2]?.close();
    // 重新走 initialDelay 100 而非 400
    vi.advanceTimersByTime(99);
    expect(ctrl.attempts()).toBe(3);
    vi.advanceTimersByTime(2);
    expect(ctrl.attempts()).toBe(4);
    ctrl.dispose();
  });

  it("visibilitychange→visible 时强制重连 + 触发 onVisible（bug #f391c55b）", () => {
    const sockets: FakeWS[] = [];
    const onVisible = vi.fn();
    // 用 ref 容器避免 TS 把 let 收窄成 null
    const listenerRef: { fn: (() => void) | null } = { fn: null };
    let hidden = false;
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      { onVisible },
      {
        initialDelayMs: 100,
        jitter: 0,
        visibility: {
          isHidden: () => hidden,
          subscribe: (l) => {
            listenerRef.fn = l;
            return () => {
              listenerRef.fn = null;
            };
          },
        },
      },
    );
    sockets[0]?.open();
    expect(ctrl.attempts()).toBe(1);
    // 切到后台
    hidden = true;
    listenerRef.fn?.();
    // hidden=true 不触发 reopen
    expect(ctrl.attempts()).toBe(1);
    expect(onVisible).not.toHaveBeenCalled();
    // 切回前台 → 立即重连 + 触发 onVisible 让 caller refetch
    hidden = false;
    listenerRef.fn?.();
    expect(onVisible).toHaveBeenCalledTimes(1);
    expect(ctrl.attempts()).toBe(2);
    // 旧 socket 被关
    expect(sockets[0]?.closed).toBe(true);
    ctrl.dispose();
    expect(listenerRef.fn).toBeNull();
  });

  it("visibility 跳过：传 visibility=null 时不订阅，避免污染无 document 环境", () => {
    const sockets: FakeWS[] = [];
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      {},
      { initialDelayMs: 50, jitter: 0, visibility: null },
    );
    sockets[0]?.open();
    ctrl.dispose();
    // dispose 不应 throw 即使没有 unsubscribe
    expect(sockets[0]?.closed).toBe(true);
  });

  it("onMessage 透传消息事件", () => {
    const sockets: FakeWS[] = [];
    const onMessage = vi.fn();
    const ctrl = startReconnectingSocket(
      () => {
        const s = new FakeWS();
        sockets.push(s);
        return s as unknown as WebSocket;
      },
      { onMessage },
      { initialDelayMs: 50, jitter: 0 },
    );
    sockets[0]?.open();
    sockets[0]?.emitMessage(JSON.stringify({ type: "agent_text", text: "hi" }));
    expect(onMessage).toHaveBeenCalledTimes(1);
    expect((onMessage.mock.calls[0]?.[0] as MessageEvent).data).toContain("agent_text");
    ctrl.dispose();
  });
});
