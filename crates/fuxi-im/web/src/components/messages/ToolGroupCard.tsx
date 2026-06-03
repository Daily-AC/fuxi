import { For, Show, createSignal, type Component } from "solid-js";
import type { ToolCallMessage } from "~/messages";
import { ToolCallCard } from "./ToolCallCard";
import styles from "./ToolGroupCard.module.css";

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `${hh}:${mm}`;
}

// 用户实测反馈（2026-05-05）：玄女一个 turn 跑 5 个 tool_call 渲染成 5 个独立卡，
// 占满屏幕，文本回复夹在中间难找。改成默认折叠成一行 header
// `[<wrench-svg> 5 个工具调用 ▸]`，点击展开看完整 ToolCallCard list。
//
// 视觉：左对齐 worker 侧（继承 row 样式），折叠态像一行 marker；展开后 cascade
// 显示原 ToolCallCard 各自的折叠/展开内部状态保留。
export const ToolGroupCard: Component<{
  items: ToolCallMessage[];
  /** 显示用角色名，从 group items[0].agent 反查（caller 传进来）。可选。*/
  role_display?: string;
}> = (props) => {
  // 单条不折叠——直接走 ToolCallCard 一致体验
  const isSingle = (): boolean => props.items.length === 1;
  const [open, setOpen] = createSignal(false);

  const summary = (): string => {
    const n = props.items.length;
    const ok = props.items.filter((t) => t.status === "ok").length;
    const err = props.items.filter((t) => t.status === "err").length;
    const running = props.items.filter((t) => t.status === "running").length;
    const parts: string[] = [`${n} 个工具调用`];
    if (running > 0) parts.push(`${running} 运行中`);
    if (err > 0) parts.push(`${err} 失败`);
    if (ok > 0 && (err > 0 || running > 0)) parts.push(`${ok} 完成`);
    return parts.join(" · ");
  };

  return (
    <Show when={!isSingle()} fallback={<ToolCallCard msg={props.items[0]!} />}>
      <div class={styles.group} data-testid="tool-group" data-count={props.items.length}>
        <button
          type="button"
          class={styles.head}
          onClick={() => setOpen(!open())}
          aria-expanded={open()}
        >
          <Show when={props.role_display}>
            <span class={styles.who}>{props.role_display}</span>
          </Show>
          <time class={styles.time}>{fmtTime(props.items[0]!.ts)}</time>
          <span class={styles.icon} aria-hidden="true">
            {/* wrench / tool SVG */}
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
            </svg>
          </span>
          <span class={styles.summary}>{summary()}</span>
          <span
            class={styles.chev}
            classList={{ [styles.chevOpen ?? ""]: open() }}
            aria-hidden="true"
          >
            ›
          </span>
        </button>
        <Show when={open()}>
          <div class={styles.list}>
            <For each={props.items}>{(item) => <ToolCallCard msg={item} />}</For>
          </div>
        </Show>
      </div>
    </Show>
  );
};
