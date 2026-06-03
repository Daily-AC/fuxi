import {
  For,
  Show,
  type Accessor,
  type Component,
  createEffect,
  createSignal,
  on,
  onCleanup,
  onMount,
} from "solid-js";
import type { Message } from "~/messages";
import { UserBubble } from "~/components/messages/UserBubble";
import { XuannvBubble } from "~/components/messages/XuannvBubble";
import { FileMessage } from "~/components/messages/FileMessage";
import { ToolCallCard } from "~/components/messages/ToolCallCard";
import { SystemMessageRow } from "~/components/messages/SystemMessageRow";
import { Mascot } from "~/components/Mascot/Mascot";
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
  // 切 tab 重 mount 时，首次 anchor 必须 instant——smooth 会从顶滑到底
  // 让用户晕（#75 用户反馈）。新消息持续追加才走 smooth。
  let hasAnchored = false;

  const isAtBottom = (): boolean => {
    if (!scrollEl) return true;
    const slack = scrollEl.scrollHeight - scrollEl.clientHeight - scrollEl.scrollTop;
    return slack < STICK_THRESHOLD;
  };

  onMount(() => {
    if (!scrollEl) return;
    scrollEl.addEventListener("scroll", () => setStickToBottom(isAtBottom()));
    // 移动端多行 composer 长大时 scrollEl 高度被 flex 挤短，最末几条消息会被
    // 推到 viewport 之下"看不见"。ResizeObserver 监听 scrollEl 尺寸变化，
    // 在 stickToBottom 状态下自动重锚到底——keystroke 级别响应。
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (!stickToBottom() || !scrollEl) return;
      scrollEl.scrollTo({ top: scrollEl.scrollHeight, behavior: "instant" });
    });
    ro.observe(scrollEl);
    onCleanup(() => ro.disconnect());
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
        const isFirst = !hasAnchored;
        queueMicrotask(() => {
          scrollEl?.scrollTo({
            top: scrollEl.scrollHeight,
            behavior: isFirst ? "instant" : "smooth",
          });
        });
        hasAnchored = true;
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
            <div class={styles.emptyMascot} aria-hidden="true">
              <Mascot state="idle" size={120} />
            </div>
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
              if (msg.kind === "file") return <FileMessage msg={msg} />;
              // bug #76：玄女自己跑工具（Bash fuxi:* / Read 等）以前在主对话页
              // 不显示——applyEvent reducer 已补 tool_call 处理，这里加渲染分支。
              if (msg.kind === "tool_call") return <ToolCallCard msg={msg} />;
              // bug #76：系统注入（[REVIEW_REQUEST]/[TRIGGER_FIRED]/[CC] 等）
              if (msg.kind === "system") return <SystemMessageRow msg={msg} />;
              return null;
            }}
          </For>
        </div>
      </Show>
    </section>
  );
};
