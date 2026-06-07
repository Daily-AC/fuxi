import { createSignal, onCleanup, onMount, type Component } from "solid-js";
import { Portal } from "solid-js/web";
import styles from "./ImageViewer.module.css";

// issue 3b5b8f25：原先图片附件点击直接 `<a target="_blank">` 走 WebView 原生打开，
// 高分辨率原图（如 1080×2400）按 1:1 像素渲染 → 用户只看到左上角一小块、无法 fit。
// 这个 lightbox 初始按 contain（fit-to-screen）显示完整图，再允许手动放大/拖动。
//
// 无第三方依赖（项目刻意保持轻量）：手写 pinch / wheel / 双击缩放 + 单指拖动。
const MAX_SCALE = 6;
const MIN_SCALE = 1;
const DOUBLE_TAP_SCALE = 2.5;

export const ImageViewer: Component<{
  src: string;
  alt: string;
  onClose: () => void;
}> = (props) => {
  const [scale, setScale] = createSignal(1);
  const [tx, setTx] = createSignal(0);
  const [ty, setTy] = createSignal(0);

  // 手势状态——用普通可变量避免每帧 signal 写入开销。
  let lastTapAt = 0;
  // 单指拖动
  let panning = false;
  let panStartX = 0;
  let panStartY = 0;
  let panOriginX = 0;
  let panOriginY = 0;
  // 双指捏合
  const pointers = new Map<number, { x: number; y: number }>();
  let pinchStartDist = 0;
  let pinchStartScale = 1;

  const clampScale = (s: number): number => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));

  // 缩放回到 1 时复位平移，避免图片被拖到视口外回不来。
  const applyScale = (next: number) => {
    const s = clampScale(next);
    setScale(s);
    if (s === MIN_SCALE) {
      setTx(0);
      setTy(0);
    }
  };

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    applyScale(scale() * (e.deltaY < 0 ? 1.15 : 0.87));
  };

  const onDblClick = () => {
    applyScale(scale() > 1 ? 1 : DOUBLE_TAP_SCALE);
  };

  const dist = (a: { x: number; y: number }, b: { x: number; y: number }): number =>
    Math.hypot(a.x - b.x, a.y - b.y);

  const onPointerDown = (e: PointerEvent) => {
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });

    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      if (a && b) {
        pinchStartDist = dist(a, b);
        pinchStartScale = scale();
        panning = false;
      }
      return;
    }

    // 双击/双触判定（移动端无 dblclick 时兜底）
    const now = e.timeStamp;
    if (now - lastTapAt < 300) {
      onDblClick();
      lastTapAt = 0;
    } else {
      lastTapAt = now;
    }

    if (scale() > 1) {
      panning = true;
      panStartX = e.clientX;
      panStartY = e.clientY;
      panOriginX = tx();
      panOriginY = ty();
    }
  };

  const onPointerMove = (e: PointerEvent) => {
    if (pointers.has(e.pointerId)) {
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    }

    if (pointers.size === 2 && pinchStartDist > 0) {
      const [a, b] = [...pointers.values()];
      if (a && b) applyScale((pinchStartScale * dist(a, b)) / pinchStartDist);
      return;
    }

    if (panning && scale() > 1) {
      setTx(panOriginX + (e.clientX - panStartX));
      setTy(panOriginY + (e.clientY - panStartY));
    }
  };

  const endPointer = (e: PointerEvent) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinchStartDist = 0;
    if (pointers.size === 0) panning = false;
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };

  onMount(() => {
    window.addEventListener("keydown", onKey);
    // 锁背景滚动，避免 viewer 打开时底下列表跟着滚。
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = prev;
    });
  });

  // 点遮罩（非图片）关闭；图片缩放时点图不关。
  const onBackdropClick = (e: MouseEvent) => {
    if (e.target === e.currentTarget) props.onClose();
  };

  return (
    <Portal>
      <div
        class={styles.backdrop}
        onClick={onBackdropClick}
        role="dialog"
        aria-modal="true"
        data-testid="image-viewer"
      >
        <button
          class={styles.close}
          onClick={() => props.onClose()}
          aria-label="关闭"
          data-testid="image-viewer-close"
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          </svg>
        </button>
        <img
          class={styles.image}
          classList={{ [styles.zoomed ?? ""]: scale() > 1 }}
          src={props.src}
          alt={props.alt}
          draggable={false}
          style={{ transform: `translate(${tx()}px, ${ty()}px) scale(${scale()})` }}
          onWheel={onWheel}
          onDblClick={onDblClick}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endPointer}
          onPointerCancel={endPointer}
        />
      </div>
    </Portal>
  );
};
