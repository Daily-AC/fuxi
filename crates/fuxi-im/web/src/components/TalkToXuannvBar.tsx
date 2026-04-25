import { createSignal, type Component } from "solid-js";
import { useApi } from "./ApiProvider";
import styles from "./TalkToXuannvBar.module.css";

// 决策 14 §A：所有 view 顶部固定"跟玄女说"输入条 → POST /api/intervene
// idle 时玄女自动 degrade 单 dispatch（决策 04），busy 时入 pending queue（M2.1）。
export const TalkToXuannvBar: Component = () => {
  const { client } = useApi();
  const [text, setText] = createSignal("");
  const [sending, setSending] = createSignal(false);
  const [hint, setHint] = createSignal<string | null>(null);

  const send = async (e?: Event): Promise<void> => {
    e?.preventDefault();
    const t = text().trim();
    if (!t || sending()) return;
    setSending(true);
    setHint(null);
    try {
      await client.intervene({ text: t });
      setText("");
    } catch (err) {
      setHint(err instanceof Error ? err.message : "发送失败");
    } finally {
      setSending(false);
    }
  };

  return (
    <form class={styles.bar} onSubmit={send} role="form" aria-label="跟玄女说">
      <textarea
        class={styles.input}
        placeholder="跟玄女说……"
        rows={1}
        value={text()}
        data-testid="xuannv-input"
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
        classList={{ [styles.sendActive ?? ""]: text().trim().length > 0 && !sending() }}
        disabled={sending() || text().trim().length === 0}
        data-testid="xuannv-send"
        aria-label="发送"
      >
        发送
      </button>
      {hint() ? (
        <div class={styles.hint} role="alert">
          {hint()}
        </div>
      ) : null}
    </form>
  );
};
