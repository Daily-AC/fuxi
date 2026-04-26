import type { Component } from "solid-js";
import type { StatusMarker } from "~/messages";
import styles from "./StatusMarkerRow.module.css";

// 状态变更 marker · 居中 muted 行 ─ text ─（task_completed / agent_idle）
export const StatusMarkerRow: Component<{ msg: StatusMarker }> = (props) => {
  return (
    <div class={styles.row} data-testid={`marker-${props.msg.id}`} role="separator">
      <span class={styles.line} aria-hidden="true" />
      <span class={styles.label}>{props.msg.text}</span>
      <span class={styles.line} aria-hidden="true" />
    </div>
  );
};
