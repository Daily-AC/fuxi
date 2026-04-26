import { Show, type Component } from "solid-js";
import type { UserMessage } from "~/messages";
import styles from "./UserBubble.module.css";

// User message · 右对齐 max-w 78%，棕调暗底 user-bubble。
// pending 时压暗一档；error 时下挂红字 inline。
export const UserBubble: Component<{ msg: UserMessage }> = (props) => {
  return (
    <div class={styles.row} data-testid="msg-user" data-msg-id={props.msg.id}>
      <div
        class={styles.bubble}
        classList={{ [styles.pending ?? ""]: Boolean(props.msg.pending) }}
      >
        {props.msg.text}
      </div>
      <Show when={props.msg.error}>
        <div class={styles.error} role="alert" data-testid="msg-user-error">
          {props.msg.error}
        </div>
      </Show>
    </div>
  );
};
