import { Show, createSignal, type Component } from "solid-js";
import type { SystemMessage } from "~/messages";
import styles from "./SystemMessageRow.module.css";

// bug #76 · 系统注入消息渲染——玄女侧（左对齐）灰底气泡 + 「系统」角标。
// 区别于右侧的 UserBubble（用户敲键盘发的）。
//
// origin 标记决定角标文案 + 颜色色调：
//   review_request   → 「📋 待审」 — accent
//   review_timeout   → 「⏱️ 呈递超时」 — warn
//   agent_dead       → 「⚠️ 门客下线」 — warn
//   trigger_fired    → 「⏰ 更漏触发」 — accent
//   carbon_copy      → 「📨 抄送」 — muted

const ORIGIN_LABEL: Record<string, string> = {
  review_request: "📋 待审",
  review_timeout: "⏱️ 呈递超时",
  agent_dead: "⚠️ 门客下线",
  trigger_fired: "⏰ 更漏触发",
  carbon_copy: "📨 抄送",
};

const COLLAPSE_THRESHOLD = 240; // 超过 240 字符折叠

export const SystemMessageRow: Component<{ msg: SystemMessage }> = (props) => {
  const [expanded, setExpanded] = createSignal(false);
  const longText = (): boolean => props.msg.text.length > COLLAPSE_THRESHOLD;
  const visibleText = (): string =>
    longText() && !expanded()
      ? `${props.msg.text.slice(0, COLLAPSE_THRESHOLD)}…`
      : props.msg.text;
  const label = (): string =>
    ORIGIN_LABEL[props.msg.origin] ?? `📨 系统消息（${props.msg.origin}）`;

  return (
    <article
      class={styles.row}
      data-testid={`system-msg-${props.msg.id}`}
      data-origin={props.msg.origin}
    >
      <header class={styles.head}>
        <span class={styles.tag}>{label()}</span>
        <span class={styles.kind}>系统注入 · 非用户语录</span>
      </header>
      <pre class={styles.body}>{visibleText()}</pre>
      <Show when={longText()}>
        <button
          type="button"
          class={styles.toggle}
          onClick={() => setExpanded(!expanded())}
          data-testid={`system-msg-toggle-${props.msg.id}`}
        >
          {expanded() ? "收起" : "展开全文"}
        </button>
      </Show>
    </article>
  );
};
