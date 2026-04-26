import { Show, type Component } from "solid-js";
import type { WorkerMessage } from "~/messages";
import { Markdown } from "../Markdown";
import styles from "./WorkerBubble.module.css";

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `${hh}:${mm}`;
}

// 私聊页门客 bubble · 跟 XuannvBubble 同结构，但 who 标签走橙色 + role_display。
// 视觉差异点都在 .name { color: var(--accent) } + .bubble bg #2A241E（暖橙微调）。
export const WorkerBubble: Component<{ msg: WorkerMessage }> = (props) => {
  return (
    <div class={styles.row} data-testid="msg-worker" data-msg-id={props.msg.id}>
      <div class={styles.head}>
        <span class={styles.name}>{props.msg.role_display ?? "门客"}</span>
        <Show when={!props.msg.streaming}>
          <time class={styles.time}>{fmtTime(props.msg.ts)}</time>
        </Show>
      </div>
      <div class={styles.bubble} data-testid="msg-worker-text">
        <Markdown source={props.msg.text} />
        <Show when={props.msg.streaming}>
          <span class="pulse-dot" data-testid="msg-streaming" aria-hidden="true" />
        </Show>
      </div>
    </div>
  );
};
