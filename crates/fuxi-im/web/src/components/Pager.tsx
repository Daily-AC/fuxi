import {
  For,
  createEffect,
  createSignal,
  on,
  onCleanup,
  onMount,
  type Component,
  type JSX,
} from "solid-js";
import styles from "./Pager.module.css";

// 横向 pager · 3 页固定。
// 设计 spec: docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §B
//
// 行为：
// - 3 页 100vw 横排，translateX 切换
// - touch swipe 切换（阈值 20% width）
// - 顶部 dots 指示当前页
// - prefers-reduced-motion → 跳过 transition（global.css 已全局关）
//
// 关键决定：dots 在 Pager 自己里渲染（不在 page 内），跨页一致；page 各自的 header 在 page 内。

const SWIPE_THRESHOLD_RATIO = 0.18; // 18% width 才算切页

export interface PagerProps {
  pages: JSX.Element[];
  /** 当前页索引（受控）。*/
  index: number;
  /** 用户切页时回调。*/
  onIndexChange(i: number): void;
  /** dots 标签（无障碍）。*/
  pageLabels: string[];
}

export const Pager: Component<PagerProps> = (props) => {
  let trackRef: HTMLDivElement | undefined;
  let rootRef: HTMLDivElement | undefined;
  const [dragX, setDragX] = createSignal(0); // 拖动时实时偏移（px，跟手）
  const [dragging, setDragging] = createSignal(false);
  let touchStartX: number | null = null;
  let touchStartY: number | null = null;
  let axisLock: "x" | "y" | null = null;

  const pageWidth = (): number => rootRef?.clientWidth ?? window.innerWidth;

  const onTouchStart = (e: TouchEvent): void => {
    const t = e.touches[0];
    if (!t) return;
    touchStartX = t.clientX;
    touchStartY = t.clientY;
    axisLock = null;
    setDragX(0);
  };

  const onTouchMove = (e: TouchEvent): void => {
    if (touchStartX === null || touchStartY === null) return;
    const t = e.touches[0];
    if (!t) return;
    const dx = t.clientX - touchStartX;
    const dy = t.clientY - touchStartY;

    // 锁轴：第一次显著偏移决定走横向（pager swipe）还是纵向（让 page 自己滚）
    if (!axisLock) {
      if (Math.abs(dx) < 10 && Math.abs(dy) < 10) return;
      axisLock = Math.abs(dx) > Math.abs(dy) ? "x" : "y";
    }
    if (axisLock === "y") return; // 让纵向滚动通过

    // 边界回弹：第一页右滑 / 末页左滑 减速
    let limited = dx;
    if (props.index === 0 && dx > 0) limited = dx * 0.35;
    if (props.index === props.pages.length - 1 && dx < 0) limited = dx * 0.35;
    setDragging(true);
    setDragX(limited);
  };

  const onTouchEnd = (): void => {
    const dx = dragX();
    const threshold = pageWidth() * SWIPE_THRESHOLD_RATIO;
    let next = props.index;
    if (dx <= -threshold && props.index < props.pages.length - 1) next = props.index + 1;
    else if (dx >= threshold && props.index > 0) next = props.index - 1;
    setDragX(0);
    setDragging(false);
    touchStartX = null;
    touchStartY = null;
    axisLock = null;
    if (next !== props.index) props.onIndexChange(next);
  };

  // 拖动期：dragging=true 时关 transition，跟手；松手 / index 变 → 加 transition 滑到位
  const transform = (): string => {
    const base = -props.index * 100;
    const offset = dragX();
    if (offset === 0) return `translateX(${base}vw)`;
    return `translateX(calc(${base}vw + ${offset}px))`;
  };

  const transition = (): string =>
    dragging() ? "none" : "transform 240ms cubic-bezier(0.2, 0.7, 0.2, 1)";

  // Re-render 时，确保 transform 跟 index 同步（边缘 case：外部 setCurrentPage）
  createEffect(
    on(
      () => props.index,
      () => {
        setDragX(0);
      },
    ),
  );

  // 防止外部 transition 在 mount 时立即触发（首屏直接 settle）
  onMount(() => {
    if (!trackRef) return;
    trackRef.style.transition = "none";
    requestAnimationFrame(() => {
      if (trackRef) trackRef.style.transition = "";
    });
  });

  // touch event 用 passive=false 让 preventDefault 能阻止纵向滚动？
  // v1 不阻止，让浏览器自然处理 axis。
  void onCleanup;

  return (
    <div class={styles.root} ref={rootRef} data-testid="pager">
      <nav class={styles.dots} aria-label="页面指示">
        <For each={props.pages}>
          {(_p, i) => (
            <button
              type="button"
              class={styles.dot}
              classList={{ [styles.dotActive ?? ""]: i() === props.index }}
              aria-label={props.pageLabels[i()] ?? `第 ${i() + 1} 页`}
              aria-current={i() === props.index ? "page" : undefined}
              data-testid={`pager-dot-${i()}`}
              onClick={() => props.onIndexChange(i() as number)}
            />
          )}
        </For>
      </nav>
      <div
        class={styles.track}
        ref={trackRef}
        style={{ transform: transform(), transition: transition() }}
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={onTouchEnd}
        onTouchCancel={onTouchEnd}
        data-testid="pager-track"
      >
        <For each={props.pages}>
          {(p, i) => (
            <section
              class={styles.page}
              data-testid={`pager-page-${i()}`}
              aria-hidden={i() !== props.index}
            >
              {p}
            </section>
          )}
        </For>
      </div>
    </div>
  );
};
