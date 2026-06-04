import { Show, type Component, type JSX } from "solid-js";
import styles from "./ScreenHeader.module.css";

// ScreenHeader · 页面顶栏（共享原语）。可选返回键 + 居中标题 + 可选 right slot。
// 返回 chevron 走 inline SVG（禁 emoji / 字符箭头）。返回键 ≥44px 触控。
export interface ScreenHeaderProps {
  title: string;
  onBack?: () => void;
  right?: JSX.Element;
}

export const ScreenHeader: Component<ScreenHeaderProps> = (props) => {
  return (
    <header class={styles.header}>
      <div class={styles.lead}>
        <Show when={props.onBack}>
          <button
            type="button"
            class={styles.back}
            data-testid="screen-back"
            aria-label="返回"
            onClick={() => props.onBack?.()}
          >
            <svg
              class={styles.chevron}
              width="11"
              height="18"
              viewBox="0 0 11 18"
              fill="none"
              aria-hidden="true"
            >
              <path
                d="M9 1.5 2 9l7 7.5"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </button>
        </Show>
      </div>
      <h1 data-testid="screen-title" class={`u-title ${styles.title}`}>
        {props.title}
      </h1>
      <div class={styles.trail}>
        <Show when={props.right}>{props.right}</Show>
      </div>
    </header>
  );
};
