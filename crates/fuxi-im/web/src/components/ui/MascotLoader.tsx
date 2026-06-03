import { type Component, For, Show } from "solid-js";
import { Mascot } from "~/components/Mascot/Mascot";
import styles from "./MascotLoader.module.css";

// MascotLoader · 全局「等待中」指示器，替代冷冰冰的 spinner。spec §5。
// think-frame 玄女 + 一圈绕行的小星点（纯 CSS 旋转），可选 label。
export interface MascotLoaderProps {
  label?: string;
  size?: number;
}

const SPARKLES = [0, 1, 2, 3, 4, 5];

export const MascotLoader: Component<MascotLoaderProps> = (props) => {
  const size = (): number => props.size ?? 96;
  return (
    <div data-testid="mascot-loader" class={styles.loader}>
      <div
        class={styles.stage}
        style={{
          width: `${size()}px`,
          height: `${size()}px`,
          // 星点轨道半径：略大于玄女半身，绕在外圈
          "--orbit-r": `${Math.round(size() * 0.58)}px`,
        }}
      >
        <Mascot state="think" size={size()} />
        <div class={styles.orbit} aria-hidden="true">
          <For each={SPARKLES}>
            {(i) => (
              <span
                class={styles.star}
                style={{ "--i": String(i), "--n": String(SPARKLES.length) }}
              />
            )}
          </For>
        </div>
      </div>
      <Show when={props.label}>
        <span class={styles.label}>{props.label}</span>
      </Show>
    </div>
  );
};
