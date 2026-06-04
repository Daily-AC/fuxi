import { Show, createSignal, type Component, type JSXElement } from "solid-js";
import type { SystemMessage } from "~/messages";
import styles from "./SystemMessageRow.module.css";

// 用户实测反馈（2026-05-05）：系统消息（agent_dead/review_request/trigger_fired/...）
// 渲染成大卡片占用太多视觉空间，重点内容失焦。改成 StatusMarkerRow 同款的
// 居中折叠 marker——默认只显一行 label `─ <clipboard-svg> 待审 ▸ ─`，点开看全文。
//
// origin 标记决定角标文案 + 颜色色调：
//   review_request   → 「<clipboard-svg> 待审」 — accent
//   review_timeout   → 「<clock-svg> 呈递超时」 — warn
//   agent_dead       → 「<warning-svg> 门客下线」 — warn
//   trigger_fired    → 「<alarm-svg> 更漏触发」 — accent
//   carbon_copy      → 「<envelope-svg> 抄送」 — muted

// ── inline SVG icon helpers (14px, currentColor stroke, no fill) ──────────

/** clipboard */
const IconClipboard = (): JSXElement => (
  <svg
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    style={{ "vertical-align": "-1px", "flex-shrink": "0" }}
  >
    <rect x="9" y="2" width="6" height="4" rx="1" />
    <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
  </svg>
);

/** hourglass / clock */
const IconClock = (): JSXElement => (
  <svg
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    style={{ "vertical-align": "-1px", "flex-shrink": "0" }}
  >
    <circle cx="12" cy="12" r="10" />
    <polyline points="12 6 12 12 16 14" />
  </svg>
);

/** warning triangle */
const IconWarning = (): JSXElement => (
  <svg
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    style={{ "vertical-align": "-1px", "flex-shrink": "0" }}
  >
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" />
    <line x1="12" y1="17" x2="12.01" y2="17" />
  </svg>
);

/** alarm / bell */
const IconAlarm = (): JSXElement => (
  <svg
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    style={{ "vertical-align": "-1px", "flex-shrink": "0" }}
  >
    <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
    <path d="M13.73 21a2 2 0 0 1-3.46 0" />
  </svg>
);

/** envelope / inbox */
const IconEnvelope = (): JSXElement => (
  <svg
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    style={{ "vertical-align": "-1px", "flex-shrink": "0" }}
  >
    <polyline points="22 12 16 12 14 15 10 15 8 12 2 12" />
    <path d="M5.45 5.11L2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
  </svg>
);

// ── origin → { icon, text } map ───────────────────────────────────────────

type OriginMeta = { icon: () => JSXElement; text: string };

const ORIGIN_META: Record<string, OriginMeta> = {
  review_request: { icon: IconClipboard, text: "待审" },
  review_timeout: { icon: IconClock, text: "呈递超时" },
  agent_dead:     { icon: IconWarning, text: "门客下线" },
  trigger_fired:  { icon: IconAlarm, text: "更漏触发" },
  carbon_copy:    { icon: IconEnvelope, text: "抄送" },
};

export const SystemMessageRow: Component<{ msg: SystemMessage }> = (props) => {
  const [expanded, setExpanded] = createSignal(false);

  const meta = (): OriginMeta =>
    ORIGIN_META[props.msg.origin] ?? { icon: IconEnvelope, text: `系统消息（${props.msg.origin}）` };

  return (
    <div
      class={styles.wrap}
      data-testid={`system-msg-${props.msg.id}`}
      data-origin={props.msg.origin}
    >
      <button
        type="button"
        class={styles.marker}
        onClick={() => setExpanded(!expanded())}
        data-testid={`system-msg-toggle-${props.msg.id}`}
        aria-expanded={expanded()}
      >
        <span class={styles.line} aria-hidden="true" />
        <span class={styles.label} style={{ display: "inline-flex", "align-items": "center", gap: "4px" }}>
          {meta().icon()}
          {meta().text}
        </span>
        <span class={styles.chevron} aria-hidden="true">
          {expanded() ? "▾" : "▸"}
        </span>
        <span class={styles.line} aria-hidden="true" />
      </button>
      <Show when={expanded()}>
        <article class={styles.detail} data-origin={props.msg.origin}>
          <header class={styles.head}>
            <span class={styles.kind}>系统注入 · 非用户语录</span>
          </header>
          <pre class={styles.body}>{props.msg.text}</pre>
        </article>
      </Show>
    </div>
  );
};
