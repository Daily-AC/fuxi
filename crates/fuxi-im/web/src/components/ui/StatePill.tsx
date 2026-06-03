import { type Component } from "solid-js";
import styles from "./StatePill.module.css";

// StatePill · 状态药丸（共享原语）。圆角 + 按 tone 软染底色。
// running=peach / done=mint / queued=lavender / warn / danger / neutral。
export type StatePillTone =
  | "running"
  | "done"
  | "queued"
  | "warn"
  | "danger"
  | "neutral";

export interface StatePillProps {
  label: string;
  tone?: StatePillTone;
}

export const StatePill: Component<StatePillProps> = (props) => {
  const tone = (): StatePillTone => props.tone ?? "neutral";
  return (
    <span data-testid="state-pill" data-tone={tone()} class={styles.pill}>
      {props.label}
    </span>
  );
};
