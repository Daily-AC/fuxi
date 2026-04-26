import {
  Show,
  createSignal,
  onCleanup,
  onMount,
  type JSX,
  type ParentComponent,
} from "solid-js";
import styles from "./BottomSheet.module.css";

// 通用底部 sheet · 复用 TasksSheet / NodesSheet。
// 行为：
//   - open=false 不渲染（不只是 hidden 防 onMount 重跑等副作用）
//   - 背景半透明黑 tap → onClose
//   - drag handle 顶 36×4 muted pill；下拉 ≥80px 触发 onClose
//   - ESC 关
//   - takes 92vh 从下滑入（CSS transform translateY 0%↔100%）
//   - prefers-reduced-motion 自动关 transition（global.css 已做）

const DISMISS_THRESHOLD_PX = 80;

export interface BottomSheetProps {
  open: boolean;
  onClose(): void;
  /** sheet 头部标题（可选，传则左上显，右挂关闭按钮）。*/
  title?: string;
  /** 标题区右侧自定义节点（如"刷新"按钮）。*/
  headerExtra?: JSX.Element;
  /** sheet body 内容。*/
  children: JSX.Element;
  /** test id 前缀（让 e2e/单测能区分 tasks-sheet vs nodes-sheet）。*/
  testId?: string;
}

export const BottomSheet: ParentComponent<BottomSheetProps> = (props) => {
  let panelRef: HTMLDivElement | undefined;
  const [dragY, setDragY] = createSignal(0);
  let touchStartY: number | null = null;

  const onTouchStart = (e: TouchEvent): void => {
    const t = e.touches[0];
    if (!t) return;
    // 仅当 sheet body 滚动到顶时才允许 drag-dismiss（防与内容滚动冲突）
    if (panelRef && panelRef.scrollTop > 0) {
      touchStartY = null;
      return;
    }
    touchStartY = t.clientY;
    setDragY(0);
  };

  const onTouchMove = (e: TouchEvent): void => {
    if (touchStartY === null) return;
    const t = e.touches[0];
    if (!t) return;
    const dy = t.clientY - touchStartY;
    if (dy > 0) setDragY(dy);
  };

  const onTouchEnd = (): void => {
    if (touchStartY === null) return;
    const dy = dragY();
    setDragY(0);
    touchStartY = null;
    if (dy >= DISMISS_THRESHOLD_PX) props.onClose();
  };

  const onKeyDown = (e: KeyboardEvent): void => {
    if (e.key === "Escape" && props.open) {
      e.preventDefault();
      props.onClose();
    }
  };

  onMount(() => {
    document.addEventListener("keydown", onKeyDown);
  });
  onCleanup(() => {
    document.removeEventListener("keydown", onKeyDown);
  });

  return (
    <Show when={props.open}>
      <div
        class={styles.backdrop}
        data-testid={props.testId ? `${props.testId}-backdrop` : "sheet-backdrop"}
        onClick={() => props.onClose()}
        role="presentation"
      />
      <div
        ref={panelRef}
        class={styles.panel}
        style={{ transform: dragY() > 0 ? `translateY(${dragY()}px)` : undefined }}
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        data-testid={props.testId ?? "sheet"}
        onTouchStart={onTouchStart}
        onTouchMove={onTouchMove}
        onTouchEnd={onTouchEnd}
        onTouchCancel={onTouchEnd}
      >
        <div class={styles.handle} aria-hidden="true" />
        <Show when={props.title}>
          <header class={styles.header}>
            <h2 class={styles.title}>{props.title}</h2>
            <div class={styles.headerRight}>
              {props.headerExtra}
              <button
                type="button"
                class={styles.closeBtn}
                onClick={() => props.onClose()}
                aria-label="关闭"
                data-testid={props.testId ? `${props.testId}-close` : "sheet-close"}
              >
                关闭
              </button>
            </div>
          </header>
        </Show>
        <div class={styles.body}>{props.children}</div>
      </div>
    </Show>
  );
};
