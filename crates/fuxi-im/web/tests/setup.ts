import "@testing-library/jest-dom/vitest";

// jsdom 没有 ResizeObserver / matchMedia / scrollIntoView —— 用 stub 兜底
class RO {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}
(globalThis as unknown as { ResizeObserver: typeof RO }).ResizeObserver = RO;

if (!window.matchMedia) {
  window.matchMedia = (q: string): MediaQueryList => ({
    matches: false,
    media: q,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  });
}

if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = function () {};
}

if (!Element.prototype.scrollTo) {
  // jsdom 不实现 scrollTo —— stub 防 Conversation 自动滚动测试爆
  Element.prototype.scrollTo = function () {};
}

// Notification API stub
if (typeof Notification === "undefined") {
  (globalThis as unknown as { Notification: { permission: string } }).Notification = {
    permission: "default",
  };
}
