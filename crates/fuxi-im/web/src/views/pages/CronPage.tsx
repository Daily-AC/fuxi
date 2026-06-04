import { For, Show, createResource, type Component, type JSX } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { EmptyState } from "~/components/ui/EmptyState";
import { StatePill } from "~/components/ui/StatePill";
import type { CronEntryView } from "~/types/api";
import styles from "./CronPage.module.css";

// 「更多 → 更漏」· v1-session17 task #9 · daimeng 奶油糖果重绘（archetype A）
//
// 列 fuxi-scheduler TriggerStore 里登记过的所有 trigger（cron / once / fs_watch
// / webhook）。仅读：CRUD 走 `fuxi cron *` CLI——PWA 不弹"+ 添加 trigger" 按钮，
// 因为 cron 表达式 + intent 在移动端键盘上手感差，TUI / 终端是更合适的工具层。
//
// RESKIN：保留全部行为 + data-testid（page-cron / cron-entry-<id>）。
// 视觉换共享原语：每 trigger 一 u-card 行卡 + 暖光闹钟 iconSlot；连失败 →
// StatePill danger；空态 EmptyState；页底 u-mesh 暖光网格。

// 闹钟 / 更漏 SVG（禁 emoji）。色由 iconSlot tone 染。
const ClockIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <circle cx="12" cy="13" r="8" />
    <path d="M12 9v4l2.5 2" stroke-linecap="round" stroke-linejoin="round" />
    <path d="M5 4 2.5 6.5M19 4l2.5 2.5" stroke-linecap="round" />
  </svg>
);

export const CronPage: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchCron());
  return (
    <div class={`u-mesh u-noise ${styles.page}`} data-testid="page-cron">
      <Show when={!data.loading} fallback={<p class={styles.muted}>加载中…</p>}>
        <Show
          when={(data()?.triggers.length ?? 0) > 0}
          fallback={
            <EmptyState
              title="更漏空空～"
              hint="还没登记任何 trigger。CRUD 走 fuxi cron 命令行。"
              mascotState="sleep"
            />
          }
        >
          <div class={styles.list}>
            <For each={data()!.triggers}>{(t) => <CronCard entry={t} />}</For>
          </div>
        </Show>
      </Show>
    </div>
  );
};

const CronCard: Component<{ entry: CronEntryView }> = (props) => {
  const failing = (): boolean => props.entry.consecutive_failures > 0;
  return (
    <article
      class={`u-card ${styles.card}`}
      classList={{ [styles.cardDisabled ?? ""]: !props.entry.enabled }}
      data-testid={`cron-entry-${props.entry.id}`}
    >
      <span
        class={styles.iconSlot}
        data-tone={failing() ? "pink" : "plain"}
        aria-hidden="true"
      >
        <ClockIcon />
      </span>
      <span class={styles.mid}>
        <span class={styles.titleLine}>
          <span class={styles.intent}>{props.entry.intent || "（无 intent）"}</span>
          <span class={styles.kind}>{props.entry.kind}</span>
        </span>
        <span class={styles.summary}>{props.entry.summary}</span>
        <span class={styles.meta}>
          <Show when={props.entry.tz}>
            {(tz) => <span>tz: {tz()}</span>}
          </Show>
          <Show when={props.entry.last_fired_at}>
            {(at) => <span>上次 fire: {at()}</span>}
          </Show>
          <Show when={!props.entry.enabled}>
            <span>· 已停</span>
          </Show>
        </span>
      </span>
      <Show when={failing()}>
        <span class={styles.trail}>
          <StatePill
            label={`连失败 ${props.entry.consecutive_failures}/${props.entry.max_failures}`}
            tone="danger"
          />
        </span>
      </Show>
    </article>
  );
};
