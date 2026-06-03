// 指数退避 + 可取消的 WebSocket 重连 helper。
//
// Bug 14B 教训：原来 ConvView/TaskView 的重连是裸 `setTimeout(open, 1500)`，
// `onCleanup` 只关 socket、不取消 timer。组件销毁后 timer 仍触发新 open，
// 新 socket 没人 close → 服务端看到 accept→close 风暴。
//
// 这里把 wire 行为收口：
// - 唯一持有 socket + timer 引用
// - `dispose()` 同时关 socket、清 timer、置 stopped 标志
// - close 后等"一定时间 + jitter"再重连；连续失败延迟翻倍直到 cap
// - 同一个 controller 不会同时存在多个 socket（race 防御）
//
// Bug #f391c55b（PWA 切 tab 回来要刷新才有新消息）：
// 浏览器把 background tab 的 WS 挂起/冻结（iOS Safari PWA 尤甚），回 tab
// 时 readyState 可能仍报 OPEN 但 send/receive 已断。`visibilitychange`
// 在 visible 时强制关旧 socket + 立刻重连（不等指数退避），并触发可选的
// `onVisible` 让 caller refetch 历史补漏（fetchHistory 已 mergeMessages 去重）。
//
// 不做：
// - heartbeat ping（后端 axum WS 默认 keepalive 已经够；要 ping 走 application 层）
// - 持久化 backoff 状态（每个 view 自己一份）

export interface ReconnectController {
  /** 当前 readyState 快照（OPEN=1 时认为已连）。*/
  readyState(): number;
  /** 显式断开 + 停止重连。组件 unmount 必须调。*/
  dispose(): void;
  /** 用作测试断言：一共开过几次 socket。*/
  attempts(): number;
}

export interface ReconnectHandlers {
  onOpen?: (sock: WebSocket) => void;
  onMessage?: (e: MessageEvent) => void;
  onClose?: () => void;
  onError?: () => void;
  /** 标签页回到前台时触发——caller 用来 refetch 历史补漏（mergeMessages 已去重）。
   *  触发时序：先于强制 reopen，让 fetch 拿当前 cursor；reopen 后新 WS 接续。 */
  onVisible?: () => void;
}

export interface ReconnectOptions {
  /** 首次失败后等多久重连，默认 200ms。*/
  initialDelayMs?: number;
  /** 退避上限，默认 5_000ms。*/
  maxDelayMs?: number;
  /** 退避乘数，默认 2。*/
  factor?: number;
  /** ±jitter 比例（0~1，默认 0.2 = ±20%）。避免多客户端同时重连。*/
  jitter?: number;
  /** 自定义 setTimeout（测试用）。*/
  setTimer?: typeof setTimeout;
  /** 自定义 clearTimeout（测试用）。*/
  clearTimer?: typeof clearTimeout;
  /** 测试用：默认绑 document.visibilitychange；传入 null 关闭可见性钩子。 */
  visibility?: VisibilityHooks | null;
}

/** 可见性钩子注入点——生产代码默认绑 document.visibilitychange，测试可注 fake。*/
export interface VisibilityHooks {
  /** 当前是否 hidden。生产 = `() => document.hidden`。 */
  isHidden(): boolean;
  /** 订阅 visibility 变化，返回取消函数。 */
  subscribe(listener: () => void): () => void;
}

const defaultVisibilityHooks: VisibilityHooks | null = (() => {
  if (typeof document === "undefined") return null;
  return {
    isHidden: () => document.hidden,
    subscribe: (listener) => {
      document.addEventListener("visibilitychange", listener);
      return () => document.removeEventListener("visibilitychange", listener);
    },
  };
})();

/** 启动一个自管理的 reconnecting socket。返回 controller。
 *  `factory` 每次调用返回**新**的 WebSocket（lib/api 的 openConvSocket 等已经满足）。
 */
export function startReconnectingSocket(
  factory: () => WebSocket,
  handlers: ReconnectHandlers,
  opts: ReconnectOptions = {},
): ReconnectController {
  const initial = opts.initialDelayMs ?? 200;
  const cap = opts.maxDelayMs ?? 5_000;
  const factor = opts.factor ?? 2;
  const jitter = opts.jitter ?? 0.2;
  const setTimer = opts.setTimer ?? setTimeout;
  const clearTimer = opts.clearTimer ?? clearTimeout;
  const visibility =
    opts.visibility === undefined ? defaultVisibilityHooks : opts.visibility;

  let stopped = false;
  let sock: WebSocket | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let nextDelay = initial;
  let attempts = 0;
  let unsubscribeVisibility: (() => void) | null = null;

  const open = (): void => {
    if (stopped) return;
    attempts += 1;
    const s = factory();
    sock = s;
    s.addEventListener("open", () => {
      if (stopped) {
        try {
          s.close();
        } catch {
          // ignore: 关 closed socket 抛是浏览器实现差异
        }
        return;
      }
      nextDelay = initial; // 成功了，重置退避
      handlers.onOpen?.(s);
    });
    s.addEventListener("message", (e) => {
      if (stopped) return;
      handlers.onMessage?.(e);
    });
    s.addEventListener("error", () => {
      if (stopped) return;
      handlers.onError?.();
    });
    s.addEventListener("close", () => {
      if (stopped) return;
      handlers.onClose?.();
      schedule();
    });
  };

  const schedule = (): void => {
    if (stopped || timer) return;
    const delta = nextDelay * jitter;
    const wait = Math.max(0, nextDelay + (Math.random() * 2 - 1) * delta);
    timer = setTimer(() => {
      timer = null;
      open();
    }, wait);
    nextDelay = Math.min(cap, Math.round(nextDelay * factor));
  };

  // 切回前台 = 强制把当前（可能已冻结但报 OPEN 的）socket 关掉立刻重连。
  // iOS Safari PWA + Chrome Android 后台 30s+ 后 WS 经常 silent freeze；
  // 单靠 close 事件触发的指数退避兜不住。
  const handleVisible = (): void => {
    if (stopped) return;
    if (visibility && visibility.isHidden()) return;
    handlers.onVisible?.();
    // 取消 pending timer
    if (timer) {
      clearTimer(timer);
      timer = null;
    }
    // 关旧 socket（如果有）—— close 事件不会再触发 schedule，因为我们立刻 open。
    if (sock) {
      try {
        sock.close();
      } catch {
        // ignore
      }
      sock = null;
    }
    nextDelay = initial;
    open();
  };

  if (visibility) {
    unsubscribeVisibility = visibility.subscribe(handleVisible);
  }

  // kick off
  open();

  return {
    readyState: () => sock?.readyState ?? WebSocket.CLOSED,
    attempts: () => attempts,
    dispose: () => {
      stopped = true;
      if (unsubscribeVisibility) {
        unsubscribeVisibility();
        unsubscribeVisibility = null;
      }
      if (timer) {
        clearTimer(timer);
        timer = null;
      }
      if (sock) {
        try {
          sock.close();
        } catch {
          // ignore
        }
        sock = null;
      }
    },
  };
}
