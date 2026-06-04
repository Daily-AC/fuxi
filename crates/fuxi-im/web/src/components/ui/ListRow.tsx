import { Show, type Component, type JSX } from "solid-js";
import styles from "./ListRow.module.css";

// ListRow · 可点列表项（共享原语）。左侧渐变圆角图标槽 + 中列标题/副标题 + 右槽。
// 有 onClick 即渲染 <button>（≥44px 触控）；无 right 时可降级渲染淡 chevron。
// 图标走 SVG slot（禁 emoji）。tone 决定图标槽渐变色调。
export interface ListRowProps {
  icon?: JSX.Element;
  title: string;
  subtitle?: string;
  right?: JSX.Element;
  onClick?: () => void;
  tone?: string;
}

export const ListRow: Component<ListRowProps> = (props) => {
  const interactive = (): boolean => props.onClick != null;
  // 内容主体复用，按 interactive 决定外壳是 button 还是 div。
  const body = (
    <>
      <Show when={props.icon}>
        <span
          class={styles.iconSlot}
          data-tone={props.tone ?? "plain"}
          aria-hidden="true"
        >
          {props.icon}
        </span>
      </Show>
      <span class={styles.mid}>
        <span data-testid="list-row-title" class={styles.title}>
          {props.title}
        </span>
        <Show when={props.subtitle}>
          <span class={styles.subtitle}>{props.subtitle}</span>
        </Show>
      </span>
      <span class={styles.trail}>
        <Show
          when={props.right}
          fallback={
            <Show when={interactive()}>
              <svg
                class={styles.chevron}
                width="8"
                height="14"
                viewBox="0 0 8 14"
                fill="none"
                aria-hidden="true"
              >
                <path
                  d="M1.5 1 6.5 7l-5 6"
                  stroke="currentColor"
                  stroke-width="1.8"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
            </Show>
          }
        >
          {props.right}
        </Show>
      </span>
    </>
  );

  return (
    <Show
      when={interactive()}
      fallback={
        <div data-testid="list-row" class={`u-card ${styles.row}`}>
          {body}
        </div>
      }
    >
      <button
        type="button"
        data-testid="list-row"
        class={`u-card ${styles.row} ${styles.tap}`}
        onClick={() => props.onClick?.()}
      >
        {body}
      </button>
    </Show>
  );
};
