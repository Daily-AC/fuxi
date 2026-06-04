import { Show, type Component, type JSX } from "solid-js";
import styles from "./Tile.module.css";

// Tile · 宫格瓦片（共享原语，用于「更多」hub）。顶部渐变圆角图标槽 + 粗 label + 淡 desc。
// 始终可点（button，≥44px 触控）。图标走 SVG slot（禁 emoji）。
export interface TileProps {
  icon?: JSX.Element;
  label: string;
  desc?: string;
  onClick?: () => void;
}

export const Tile: Component<TileProps> = (props) => {
  return (
    <button
      type="button"
      data-testid="tile"
      class={`u-card ${styles.tile}`}
      onClick={() => props.onClick?.()}
    >
      <Show when={props.icon}>
        <span class={styles.iconSlot} aria-hidden="true">
          {props.icon}
        </span>
      </Show>
      <span data-testid="tile-label" class={styles.label}>
        {props.label}
      </span>
      <Show when={props.desc}>
        <span class={styles.desc}>{props.desc}</span>
      </Show>
    </button>
  );
};
