import { type Component, type JSX, Show } from "solid-js";
import { Mascot } from "~/components/Mascot/Mascot";
import type { MascotStateKind } from "~/components/Mascot/mascotMachine";
import styles from "./EmptyState.module.css";

// EmptyState · 友好空态占位（替代冷「暂无数据」）。spec §5。
// 默认 sleep 态（无 blink 定时器，单测无残留 handle）。
export interface EmptyStateProps {
  title: string;
  hint?: string;
  mascotState?: MascotStateKind;
  action?: JSX.Element;
}

export const EmptyState: Component<EmptyStateProps> = (props) => {
  const state = (): MascotStateKind => props.mascotState ?? "sleep";
  return (
    <div data-testid="empty-state" class={styles.empty}>
      <Mascot state={state()} size={88} />
      <p class={styles.title}>{props.title}</p>
      <Show when={props.hint}>
        <p class={styles.hint}>{props.hint}</p>
      </Show>
      <Show when={props.action}>
        <div class={styles.action}>{props.action}</div>
      </Show>
    </div>
  );
};
