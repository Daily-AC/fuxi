import { Show, type Component, createSignal, type Accessor } from "solid-js";
import styles from "./Conversation.module.css";

// 主屏：纯对话区。阶段 1 仅空态 + 骨架；下阶段接消息流。
// chat 区 padding 20px 横 / 16 竖，gap 14px。messages 来自 props 让父级控制。
export interface ConversationProps {
  /** 阶段 1 暂时只用 length 判空态，下阶段接真消息 union 类型。*/
  messages: Accessor<unknown[]>;
}

export const Conversation: Component<ConversationProps> = (props) => {
  const [_atBottom] = createSignal(true);
  void _atBottom;
  return (
    <section class={styles.root} data-testid="conversation">
      <Show
        when={props.messages().length > 0}
        fallback={
          <div class={styles.empty} data-testid="conversation-empty">
            <p class={styles.emptyTitle}>玄女在线</p>
            <p class={styles.emptyHint}>跟她说点啥</p>
          </div>
        }
      >
        <div class={styles.stream} data-testid="conversation-stream">
          {/* 阶段 2 替换为 messages.map */}
        </div>
      </Show>
    </section>
  );
};
