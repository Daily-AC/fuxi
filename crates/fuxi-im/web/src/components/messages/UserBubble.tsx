import { For, Show, type Component } from "solid-js";
import type { UserMessage } from "~/messages";
import { Markdown } from "../Markdown";
import { AttachmentChip } from "./AttachmentChip";
import styles from "./UserBubble.module.css";

// bug #77：用户实测「@不显示在气泡里」。简化版渲染——气泡顶上一行 chip
// `@<id_short>`，让用户至少看见发给谁。等以后 reducer 注入 role 时升级 chip 颜色。
function shortMentionLabel(id: string): string {
  const trimmed = id.startsWith("agent-") ? id.slice("agent-".length) : id;
  return `@${trimmed.slice(0, 6)}`;
}

// User message · 右对齐 max-w 78%，棕调暗底 user-bubble。
// pending 时压暗一档；error 时下挂红字 inline。
// 阶段 3：text 走 markdown 渲染；attachments 在 bubble 内挂 chip。
export const UserBubble: Component<{ msg: UserMessage }> = (props) => {
  const hasText = (): boolean => Boolean(props.msg.text && props.msg.text.length > 0);
  const hasAttachments = (): boolean =>
    Array.isArray(props.msg.attachments) && props.msg.attachments.length > 0;
  const hasMentions = (): boolean =>
    Array.isArray(props.msg.mentions) && props.msg.mentions.length > 0;

  return (
    <div class={styles.row} data-testid="msg-user" data-msg-id={props.msg.id}>
      <div
        class={styles.bubble}
        classList={{ [styles.pending ?? ""]: Boolean(props.msg.pending) }}
      >
        <Show when={hasMentions()}>
          <div class={styles.mentions} data-testid="msg-user-mentions">
            <For each={props.msg.mentions}>
              {(id) => <span class={styles.mentionChip}>{shortMentionLabel(id)}</span>}
            </For>
          </div>
        </Show>
        <Show when={hasText()}>
          <Markdown source={props.msg.text} />
        </Show>
        <Show when={hasAttachments()}>
          <div class={styles.attachments}>
            <For each={props.msg.attachments}>{(u) => <AttachmentChip upload={u} />}</For>
          </div>
        </Show>
      </div>
      <Show when={props.msg.error}>
        <div class={styles.error} role="alert" data-testid="msg-user-error">
          {props.msg.error}
        </div>
      </Show>
    </div>
  );
};
