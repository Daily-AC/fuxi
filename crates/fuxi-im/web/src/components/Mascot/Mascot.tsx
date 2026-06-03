import { createEffect, createSignal, onCleanup, type Component } from "solid-js";
import type { MascotStateKind } from "./mascotMachine";
import styles from "./Mascot.module.css";

export interface MascotProps {
  state: MascotStateKind;
  size: number;
  onPoke?: () => void;
  draggable?: boolean;
}

// 已有的 webp 帧文件名（public/mascot/xuannv-<frame>.webp）。
const FRAMES = new Set<string>([
  "idle",
  "blink",
  "happy",
  "think",
  "talk",
  "wave",
  "sleep",
  "surprise",
]);

// state → 帧文件名映射：
// - poke 没有专属帧，借 happy
// - 其余 state 用自己同名帧
// - 没有同名帧的（理论上不该出现）退回 idle
function frameForState(state: MascotStateKind): string {
  if (state === "poke") return "happy";
  if (FRAMES.has(state)) return state;
  return "idle";
}

function frameSrc(frame: string): string {
  return `/mascot/xuannv-${frame}.webp`;
}

function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export const Mascot: Component<MascotProps> = (props) => {
  // 眨眼信号：仅在 idle 时短暂把显示帧切到 blink；非 idle 由 prop 全权决定。
  const [blinking, setBlinking] = createSignal(false);
  const [wobbling, setWobbling] = createSignal(false);
  // 拖拽位移
  const [drag, setDrag] = createSignal<{ x: number; y: number } | null>(null);

  const reduce = prefersReducedMotion();

  // 当前要显示的帧：idle 且正在眨眼 → blink，否则按 state 映射。
  const shownFrame = (): string => {
    if (props.state === "idle" && blinking()) return "blink";
    return frameForState(props.state);
  };

  // idle 眨眼定时器：2-4s 随机间隔，眨 ~150ms。reduced-motion 下不启动。
  createEffect(() => {
    if (reduce) return;
    if (props.state !== "idle") {
      setBlinking(false);
      return;
    }
    let blinkTimer: ReturnType<typeof setTimeout> | null = null;
    let unblinkTimer: ReturnType<typeof setTimeout> | null = null;
    let alive = true;

    const scheduleNext = (): void => {
      const delay = 2000 + Math.random() * 2000;
      blinkTimer = setTimeout(() => {
        if (!alive) return;
        setBlinking(true);
        unblinkTimer = setTimeout(() => {
          if (!alive) return;
          setBlinking(false);
          scheduleNext();
        }, 150);
      }, delay);
    };
    scheduleNext();

    onCleanup(() => {
      alive = false;
      if (blinkTimer) clearTimeout(blinkTimer);
      if (unblinkTimer) clearTimeout(unblinkTimer);
    });
  });

  const handleClick = (): void => {
    props.onPoke?.();
    if (reduce) return;
    setWobbling(true);
  };

  // 拖拽：pointer down→move→up，松手归位。minimal 实现。
  let pointerActive = false;
  let startX = 0;
  let startY = 0;

  const handlePointerDown = (e: PointerEvent): void => {
    if (!props.draggable) return;
    pointerActive = true;
    startX = e.clientX;
    startY = e.clientY;
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  };
  const handlePointerMove = (e: PointerEvent): void => {
    if (!pointerActive) return;
    setDrag({ x: e.clientX - startX, y: e.clientY - startY });
  };
  const handlePointerUp = (e: PointerEvent): void => {
    if (!pointerActive) return;
    pointerActive = false;
    setDrag(null);
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
  };

  // 呼吸：idle/think 这类静态态轻微起伏；瞬时/对话态不呼吸（动作本身够活）。
  const breathing = (): boolean =>
    !reduce && (props.state === "idle" || props.state === "think" || props.state === "sleep");

  const holderTransform = (): string | undefined => {
    const d = drag();
    return d ? `translate(${d.x}px, ${d.y}px)` : undefined;
  };

  return (
    <div
      data-testid="mascot"
      class={styles.mascot}
      classList={{
        [styles.breathe ?? ""]: breathing(),
        [styles.wobble ?? ""]: wobbling(),
        [styles.draggable ?? ""]: !!props.draggable,
      }}
      style={{
        width: `${props.size}px`,
        height: "auto",
        transform: holderTransform(),
      }}
      onClick={handleClick}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
      onAnimationEnd={() => setWobbling(false)}
    >
      <img
        data-testid="mascot-img"
        class={styles.img}
        src={frameSrc(shownFrame())}
        alt="玄女"
        draggable={false}
        width={props.size}
      />
    </div>
  );
};
