import { Show, createSignal, type Component } from "solid-js";
import type { ToolCallMessage } from "~/messages";
import { formatDuration } from "~/lib/format-task";
import styles from "./ToolCallCard.module.css";

// 工具调用折叠卡 · spec §私聊页 渲染规则
//   折叠态：[▸ Bash · grep ...   ✓ 0.4s]
//   tap 展开：output 前 240px 高度滚动；ToolFinished 没拿到 output → 隐展开按钮
export const ToolCallCard: Component<{ msg: ToolCallMessage }> = (props) => {
  const [open, setOpen] = createSignal(false);
  const hasOutput = (): boolean =>
    typeof props.msg.output === "string" && props.msg.output.length > 0;

  const statusLabel = (): string => {
    if (props.msg.status === "running") return "运行中";
    if (props.msg.status === "ok") return "完成";
    return "失败";
  };

  return (
    <article
      class={styles.card}
      data-testid={`tool-card-${props.msg.id}`}
      data-status={props.msg.status}
    >
      <button
        type="button"
        class={styles.head}
        onClick={() => hasOutput() && setOpen(!open())}
        aria-expanded={open()}
        aria-label={`${props.msg.tool} ${statusLabel()}`}
        disabled={!hasOutput()}
      >
        <span
          class={styles.chev}
          classList={{ [styles.chevOpen ?? ""]: open() }}
          aria-hidden="true"
        >
          ›
        </span>
        <span class={styles.tool}>{props.msg.tool}</span>
        <Show when={props.msg.args_summary}>
          <span class={styles.args}>{props.msg.args_summary}</span>
        </Show>
        <Show when={props.msg.status === "running"}>
          <span class={styles.statusRunning}>运行中</span>
        </Show>
        <Show when={props.msg.status === "ok"}>
          <span class={styles.statusOk}>完成</span>
        </Show>
        <Show when={props.msg.status === "err"}>
          <span class={styles.statusErr}>失败</span>
        </Show>
        <Show when={typeof props.msg.duration_ms === "number" && props.msg.duration_ms > 0}>
          <time class={styles.duration}>{formatDuration(props.msg.duration_ms ?? 0)}</time>
        </Show>
      </button>
      <Show when={open() && hasOutput()}>
        <div class={styles.body} data-testid={`tool-card-output-${props.msg.id}`}>
          <pre class={styles.output}>{props.msg.output}</pre>
        </div>
      </Show>
    </article>
  );
};
