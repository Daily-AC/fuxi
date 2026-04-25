import { Show, type Component, createSignal } from "solid-js";
import type { EventKind } from "~/types/events";
import { shortAgentId } from "~/lib/format";
import { StreamingText } from "./StreamingText";
import styles from "./EventLine.module.css";

// 单条事件渲染。tool_call / diff 默认折叠，点开展开。
// 不抄气泡视觉 —— 用细分隔 + 左侧细竖条标识发言主体。
export const EventLine: Component<{ ev: EventKind; streaming?: boolean }> = (props) => {
  const [open, setOpen] = createSignal(false);
  const ev = (): EventKind => props.ev;

  const kind = (): string => (ev() as { type: string }).type;

  return (
    <div class={styles.row} data-kind={kind()} data-testid={`ev-${kind()}`}>
      <Show when={kind() === "user_message"}>
        <div class={styles.user}>
          <span class={styles.label}>你</span>
          <span class={styles.userBody}>{(ev() as { text: string }).text}</span>
        </div>
      </Show>

      <Show when={kind() === "agent_text" || kind() === "agent_responded"}>
        <div class={styles.agent}>
          <span class={styles.label}>
            <span class="agent-id">{shortAgentId((ev() as { agent: string }).agent ?? "")}</span>
          </span>
          <div class={styles.body}>
            <StreamingText
              text={(ev() as { text: string }).text}
              streaming={Boolean(props.streaming)}
            />
          </div>
        </div>
      </Show>

      <Show when={kind() === "agent_text_delta"}>
        <div class={styles.agent}>
          <span class={styles.label}>
            <span class="agent-id">{shortAgentId((ev() as { agent: string }).agent ?? "")}</span>
          </span>
          <div class={styles.body}>
            <StreamingText
              text={(ev() as { delta: string }).delta}
              streaming
            />
          </div>
        </div>
      </Show>

      <Show when={kind() === "task_created"}>
        <div class={styles.system}>
          创建任务：
          <span class={styles.systemEmph}>{(ev() as { title: string }).title}</span>
        </div>
      </Show>

      <Show when={kind() === "task_dispatched"}>
        <div class={styles.system}>
          派往
          <span class="agent-id">{shortAgentId((ev() as { agent: string }).agent ?? "")}</span>
        </div>
      </Show>

      <Show when={kind() === "task_completed"}>
        <div class={styles.systemDone}>
          {(ev() as { ok: boolean }).ok ? "任务完成" : "任务结束（未成功）"}
          <Show when={(ev() as { summary?: string }).summary}>
            <span class={styles.systemEmph}>· {(ev() as { summary?: string }).summary}</span>
          </Show>
        </div>
      </Show>

      <Show when={kind() === "task_failed"}>
        <div class={styles.systemFail}>失败：{(ev() as { error: string }).error}</div>
      </Show>

      <Show when={kind() === "tool_call"}>
        <div class={styles.tool}>
          <button
            class={styles.toolHead}
            onClick={() => setOpen(!open())}
            aria-expanded={open()}
          >
            <span class={styles.toolMark} aria-hidden="true">
              {open() ? "▾" : "▸"}
            </span>
            <span class={styles.label}>
              <span class="agent-id">{shortAgentId((ev() as { agent: string }).agent ?? "")}</span>
            </span>
            <span class={styles.toolName}>调用 {(ev() as { tool: string }).tool}</span>
          </button>
          <Show when={open()}>
            <pre class={styles.toolBody}>
              {JSON.stringify((ev() as { input?: unknown }).input ?? null, null, 2)}
            </pre>
          </Show>
        </div>
      </Show>

      <Show when={kind() === "tool_result"}>
        <div class={styles.tool}>
          <button
            class={styles.toolHead}
            onClick={() => setOpen(!open())}
            aria-expanded={open()}
          >
            <span class={styles.toolMark} aria-hidden="true">
              {open() ? "▾" : "▸"}
            </span>
            <span class={styles.toolName}>
              {(ev() as { ok: boolean }).ok ? "结果" : "错误"} ·{" "}
              {(ev() as { tool: string }).tool}
            </span>
          </button>
          <Show when={open()}>
            <pre class={styles.toolBody}>
              {typeof (ev() as { output?: unknown }).output === "string"
                ? ((ev() as { output: string }).output as string)
                : JSON.stringify((ev() as { output?: unknown }).output ?? null, null, 2)}
            </pre>
          </Show>
        </div>
      </Show>

      <Show when={kind() === "diff"}>
        <div class={styles.tool}>
          <button
            class={styles.toolHead}
            onClick={() => setOpen(!open())}
            aria-expanded={open()}
          >
            <span class={styles.toolMark} aria-hidden="true">
              {open() ? "▾" : "▸"}
            </span>
            <span class={styles.toolName}>
              改动 <span class="agent-id">{(ev() as { path: string }).path}</span>
            </span>
          </button>
          <Show when={open()}>
            <pre class={styles.toolBody}>
              {(ev() as { after?: string }).after ?? "(无后内容)"}
            </pre>
          </Show>
        </div>
      </Show>

      <Show
        when={
          ![
            "user_message",
            "agent_text",
            "agent_responded",
            "agent_text_delta",
            "task_created",
            "task_dispatched",
            "task_completed",
            "task_failed",
            "tool_call",
            "tool_result",
            "diff",
          ].includes(kind())
        }
      >
        <div class={styles.system}>{kind()}</div>
      </Show>
    </div>
  );
};
