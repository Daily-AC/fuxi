import { createSignal, type Component } from "solid-js";
import styles from "./Composer.module.css";

// 底部 sticky composer · IM 标准位置。
// Enter 提交（无 shift）；shift+Enter 换行；空字符不提交；submitting 期 disabled 防双击。
// 视觉：pill input + pill 发送按钮，accent active / muted disabled 双态。
export interface ComposerProps {
  /** 提交回调，父级负责 optimistic 渲染 + intervene 调用。返回 Promise 让父级控制 loading。*/
  onSubmit: (text: string) => Promise<void>;
  placeholder?: string;
  /** 父级强制 disabled（如 ws 未就绪）。*/
  disabled?: boolean;
}

export const Composer: Component<ComposerProps> = (props) => {
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const send = async (e?: Event): Promise<void> => {
    e?.preventDefault();
    const t = text().trim();
    if (!t || busy()) return;
    setBusy(true);
    try {
      await props.onSubmit(t);
      setText("");
    } finally {
      setBusy(false);
    }
  };

  const canSend = (): boolean => !busy() && !props.disabled && text().trim().length > 0;

  return (
    <form class={styles.composer} onSubmit={send} role="form" aria-label="跟玄女说">
      <textarea
        class={styles.input}
        rows={1}
        placeholder={props.placeholder ?? "跟玄女说……"}
        value={text()}
        data-testid="composer-input"
        disabled={busy() || props.disabled}
        onInput={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void send();
          }
        }}
      />
      <button
        type="submit"
        class={styles.send}
        classList={{ [styles.sendActive ?? ""]: canSend() }}
        disabled={!canSend()}
        data-testid="composer-send"
        aria-label="发送"
      >
        发送
      </button>
    </form>
  );
};
