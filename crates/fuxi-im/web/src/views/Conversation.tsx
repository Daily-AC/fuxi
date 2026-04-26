import {
  For,
  Show,
  type Accessor,
  type Component,
  createEffect,
  createSignal,
  on,
  onMount,
} from "solid-js";
import type { Message } from "~/messages";
import { UserBubble } from "~/components/messages/UserBubble";
import { XuannvBubble } from "~/components/messages/XuannvBubble";
import styles from "./Conversation.module.css";

// 主屏 chat scroll · padding 16/20，gap 14。空态居中淡字。
// 用户在底部时自动跟随；上滑回看时不打扰（避免 task #14 同款体验问题）。
export interface ConversationProps {
  messages: Accessor<Message[]>;
}

const STICK_THRESHOLD = 80; // px

export const Conversation: Component<ConversationProps> = (props) => {
  let scrollEl: HTMLDivElement | undefined;
  const [stickToBottom, setStickToBottom] = createSignal(true);

  const isAtBottom = (): boolean => {
    if (!scrollEl) return true;
    const slack = scrollEl.scrollHeight - scrollEl.clientHeight - scrollEl.scrollTop;
    return slack < STICK_THRESHOLD;
  };

  onMount(() => {
    if (!scrollEl) return;
    scrollEl.addEventListener("scroll", () => setStickToBottom(isAtBottom()));
  });

  createEffect(
    on(
      () => {
        const list = props.messages();
        const last = list[list.length - 1];
        const lastText = last && "text" in last ? last.text.length : 0;
        return [list.length, lastText] as const;
      },
      () => {
        if (!stickToBottom() || !scrollEl) return;
        queueMicrotask(() => {
          scrollEl?.scrollTo({ top: scrollEl.scrollHeight, behavior: "smooth" });
        });
      },
      { defer: true },
    ),
  );

  return (
    <section
      ref={scrollEl}
      class={styles.root}
      data-testid="conversation"
    >
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
          <For each={props.messages()}>
            {(msg) => {
              if (msg.kind === "user") return <UserBubble msg={msg} />;
              if (msg.kind === "xuannv") return <XuannvBubble msg={msg} />;
              return null;
            }}
          </For>
        </div>
      </Show>
    </section>
  );
};
