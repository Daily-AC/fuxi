import { createEffect, createSignal, type Component, on } from "solid-js";
import styles from "./StreamingText.module.css";

// 字符级流式渲染：拿到完整文本后逐字展开。
// 实战中后端会按 delta 推；前端把累积文本传进来，组件负责"上次到现在"的字符 reveal。
function prefersReducedMotion(): boolean {
  if (typeof window === "undefined" || !window.matchMedia) return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export const StreamingText: Component<{
  text: string;
  /** 是否仍在流（true 时显示尾部光标） */
  streaming?: boolean;
  /** 每字间隔 ms，默认 18ms */
  charDelay?: number;
}> = (props) => {
  const [visible, setVisible] = createSignal(0);
  let timer: ReturnType<typeof setTimeout> | null = null;

  const tick = (target: number, delay: number): void => {
    if (timer) clearTimeout(timer);
    if (visible() >= target) return;
    setVisible(visible() + 1);
    if (visible() < target) {
      timer = setTimeout(() => tick(target, delay), delay);
    }
  };

  createEffect(
    on(
      () => props.text,
      (t) => {
        const target = t.length;
        if (target < visible()) {
          // 文本被替换/截短：直接对齐，不做倒退动画
          setVisible(target);
          return;
        }
        // reduced-motion：直接落到完整文本，跳过逐字 tick
        if (prefersReducedMotion()) {
          setVisible(target);
          return;
        }
        tick(target, props.charDelay ?? 18);
      },
    ),
  );

  return (
    <span class={styles.wrap}>
      <span class={styles.body}>{props.text.slice(0, visible())}</span>
      {props.streaming || visible() < props.text.length ? (
        <span class="stream-cursor" aria-hidden="true" />
      ) : null}
    </span>
  );
};
