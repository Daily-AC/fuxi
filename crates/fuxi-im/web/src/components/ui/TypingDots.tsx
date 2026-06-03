import { type Component } from "solid-js";
import styles from "./TypingDots.module.css";

// TypingDots · 流式输出时的三点跳动。spec §5。
export interface TypingDotsProps {
  class?: string;
}

export const TypingDots: Component<TypingDotsProps> = (props) => {
  return (
    <span data-testid="typing-dots" class={`${styles.dots} ${props.class ?? ""}`} aria-hidden="true">
      <span class={styles.dot} />
      <span class={styles.dot} />
      <span class={styles.dot} />
    </span>
  );
};
