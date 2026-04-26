import { Show, type Component } from "solid-js";
import type { ThinkingMessage } from "~/messages";
import { formatDuration } from "~/lib/format-task";
import styles from "./ThinkingRow.module.css";

// thinking 折叠条 · 灰色斜体 `▸ 思考 Ns`
// streaming 时持续显 "思考中"，完结后显 duration。
// v1 不展开（CoT 后端不发；将来 sonnet/opus 加 thinking content 再扩）。
export const ThinkingRow: Component<{ msg: ThinkingMessage }> = (props) => {
  return (
    <div
      class={styles.row}
      classList={{ [styles.streaming ?? ""]: props.msg.streaming }}
      data-testid={`thinking-${props.msg.id}`}
      data-streaming={props.msg.streaming ? "true" : "false"}
    >
      <span class={styles.chev} aria-hidden="true">
        ›
      </span>
      <Show
        when={props.msg.streaming}
        fallback={
          <span>
            思考{" "}
            <Show when={typeof props.msg.duration_ms === "number" && props.msg.duration_ms > 0}>
              {formatDuration(props.msg.duration_ms ?? 0)}
            </Show>
          </span>
        }
      >
        <span>思考中…</span>
      </Show>
    </div>
  );
};
