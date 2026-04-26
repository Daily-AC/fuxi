import { Show, type Component } from "solid-js";
import type { XuannvMessage } from "~/messages";
import { Markdown } from "../Markdown";
import styles from "./XuannvBubble.module.css";

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `${hh}:${mm}`;
}

// 玄女 message · 左对齐 max-w 78%，name "玄女" xuannv 紫 13px semibold，time 11px muted 右挂。
// streaming=true → bubble 末尾 pulse-dot。
// 阶段 3：text 走 Markdown（streaming 时也每 delta 重 parse）。
export const XuannvBubble: Component<{ msg: XuannvMessage }> = (props) => {
  return (
    <div class={styles.row} data-testid="msg-xuannv" data-msg-id={props.msg.id}>
      <div class={styles.head}>
        <span class={styles.name}>玄女</span>
        <Show when={!props.msg.streaming}>
          <time class={styles.time}>{fmtTime(props.msg.ts)}</time>
        </Show>
      </div>
      <div class={styles.bubble} data-testid="msg-xuannv-text">
        <Markdown source={props.msg.text} />
        <Show when={props.msg.streaming}>
          <span class="pulse-dot" data-testid="msg-streaming" aria-hidden="true" />
        </Show>
      </div>
    </div>
  );
};
