import { Show, type Component } from "solid-js";
import type { InlineFileMessage } from "~/messages";
import { Markdown } from "../Markdown";
import styles from "./InlineFileCard.module.css";

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `${hh}:${mm}`;
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

// P2.7 · 门客 inline 文件推送卡片。结构：
//   ─────────────────────────────────────
//   鲁班 · report.md (text/markdown 1.2 KB)        14:32
//   ─────────────────────────────────────
//   <Markdown 渲染 body 或 <pre>>
//   ─────────────────────────────────────
//
// 视觉跟 WorkerBubble 同色温（橙系），但加 .filename header 和分隔线。
// text/plain 走 <pre> 保格式；text/markdown 走 <Markdown />。
export const InlineFileCard: Component<{ msg: InlineFileMessage }> = (props) => {
  const isMd = (): boolean => props.msg.mime === "text/markdown";
  const sizeBytes = (): number => new Blob([props.msg.body]).size;

  return (
    <div class={styles.row} data-testid="msg-inline-file" data-msg-id={props.msg.id}>
      <div class={styles.head}>
        <span class={styles.name}>{props.msg.role_display ?? "门客"}</span>
        <span class={styles.sep}>·</span>
        <span class={styles.filename}>{props.msg.filename}</span>
        <span class={styles.meta}>
          {props.msg.mime} · {fmtSize(sizeBytes())}
        </span>
        <time class={styles.time}>{fmtTime(props.msg.ts)}</time>
      </div>
      <div class={styles.card}>
        <Show when={isMd()} fallback={<pre class={styles.pre}>{props.msg.body}</pre>}>
          <Markdown source={props.msg.body} />
        </Show>
      </div>
    </div>
  );
};
