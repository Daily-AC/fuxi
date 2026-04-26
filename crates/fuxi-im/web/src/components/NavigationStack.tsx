import {
  Show,
  createSignal,
  onCleanup,
  onMount,
  type Component,
  type JSX,
} from "solid-js";
import styles from "./NavigationStack.module.css";

// iOS 风 NavigationStack · v1 一层栈（pager 在底，可选私聊页 push 在上面）。
// 设计 spec: docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §B 私聊页
//
// 行为：
// - pushed=true 时 modal slide-from-right（240ms ease）盖在 base 之上
// - 顶栏 "‹ 玄女" 由 pushed page 自己渲染，调 onPop 触发 pop
// - iOS 边缘左滑 pop（touchstart x < EDGE_PX → 跟手 → 释放 ≥ 35% width 触发 pop）
// - prefers-reduced-motion 下 transition 由 global.css 关掉

const EDGE_PX = 24; // 左缘多少 px 内触发 edge swipe
const POP_THRESHOLD_RATIO = 0.35;

export interface NavigationStackProps {
  base: JSX.Element;
  /** 顶层 push 的内容；undefined = 没 push。*/
  top?: JSX.Element;
  /** edge swipe 触发的 pop 回调；外部 navPop 也调它。*/
  onPop(): void;
}

export const NavigationStack: Component<NavigationStackProps> = (props) => {
  let topRef: HTMLDivElement | undefined;
  const [dragX, setDragX] = createSignal(0);
  const [dragging, setDragging] = createSignal(false);
  let touchStartX: number | null = null;
  let touchStartY: number | null = null;
  let activeEdge = false;
  let axisLock: "x" | "y" | null = null;

  const screenWidth = (): number => topRef?.clientWidth ?? window.innerWidth;

  const onTouchStart = (e: TouchEvent): void => {
    const t = e.touches[0];
    if (!t) return;
    // 仅当起点在左缘且确实有 top 时进入 edge swipe 跟手
    if (props.top === undefined) return;
    if (t.clientX > EDGE_PX) return;
    touchStartX = t.clientX;
    touchStartY = t.clientY;
    activeEdge = true;
    axisLock = null;
    setDragX(0);
  };

  const onTouchMove = (e: TouchEvent): void => {
    if (!activeEdge || touchStartX === null || touchStartY === null) return;
    const t = e.touches[0];
    if (!t) return;
    const dx = t.clientX - touchStartX;
    const dy = t.clientY - touchStartY;
    if (!axisLock) {
      if (Math.abs(dx) < 8 && Math.abs(dy) < 8) return;
      axisLock = Math.abs(dx) > Math.abs(dy) ? "x" : "y";
    }
    if (axisLock === "y") {
      activeEdge = false;
      return;
    }
    if (dx <= 0) return; // 只跟右滑
    setDragging(true);
    setDragX(dx);
  };

  const onTouchEnd = (): void => {
    if (!activeEdge) return;
    const dx = dragX();
    const should = dx >= screenWidth() * POP_THRESHOLD_RATIO;
    setDragX(0);
    setDragging(false);
    activeEdge = false;
    touchStartX = null;
    touchStartY = null;
    axisLock = null;
    if (should) props.onPop();
  };

  // ESC 关 top（无障碍 / 桌面友好）
  const onKeyDown = (e: KeyboardEvent): void => {
    if (e.key === "Escape" && props.top !== undefined) {
      e.preventDefault();
      props.onPop();
    }
  };

  onMount(() => document.addEventListener("keydown", onKeyDown));
  onCleanup(() => document.removeEventListener("keydown", onKeyDown));

  const transform = (): string => {
    const dx = dragX();
    return dx > 0 ? `translateX(${dx}px)` : "";
  };

  const transition = (): string =>
    dragging() ? "none" : "transform 240ms cubic-bezier(0.2, 0.7, 0.2, 1)";

  return (
    <div class={styles.root} data-testid="nav-stack">
      <div class={styles.base} data-testid="nav-base">
        {props.base}
      </div>
      <Show when={props.top !== undefined}>
        <div
          ref={topRef}
          class={styles.top}
          style={{ transform: transform(), transition: transition() }}
          data-testid="nav-top"
          onTouchStart={onTouchStart}
          onTouchMove={onTouchMove}
          onTouchEnd={onTouchEnd}
          onTouchCancel={onTouchEnd}
        >
          {props.top}
        </div>
      </Show>
    </div>
  );
};
