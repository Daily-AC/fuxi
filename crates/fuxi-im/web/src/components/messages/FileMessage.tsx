import { For, Show, type Component } from "solid-js";
import type { FileMessage as FileMsgType } from "~/messages";
import { AttachmentChip } from "./AttachmentChip";
import { Markdown } from "../Markdown";
import styles from "./FileMessage.module.css";

// 文件消息 · 一条 message 包 N 个附件 + 可选 caption。
// 阶段 3 简化：role=user 用棕底，其余（玄女/门客）用 surface 底。
function isUser(role: FileMsgType["role"]): boolean {
  return role === "user";
}

export const FileMessage: Component<{ msg: FileMsgType }> = (props) => {
  return (
    <div
      class={styles.row}
      classList={{ [styles.right ?? ""]: isUser(props.msg.role) }}
      data-testid="msg-file"
      data-msg-id={props.msg.id}
      data-role={props.msg.role}
    >
      <div
        class={styles.bubble}
        classList={{
          [styles.userBubble ?? ""]: isUser(props.msg.role),
          [styles.agentBubble ?? ""]: !isUser(props.msg.role),
        }}
      >
        <Show when={props.msg.caption}>
          <div class={styles.caption}>
            <Markdown source={props.msg.caption ?? ""} />
          </div>
        </Show>
        <div class={styles.attachments}>
          <For each={props.msg.attachments}>
            {(u) => <AttachmentChip upload={u} />}
          </For>
        </div>
      </div>
    </div>
  );
};
