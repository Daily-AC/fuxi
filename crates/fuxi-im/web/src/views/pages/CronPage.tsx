import { For, Show, createResource, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import type { CronEntryView } from "~/types/api";
import styles from "./CronPage.module.css";

// 「更多 → 更漏」· v1-session17 task #9
//
// 列 fuxi-scheduler TriggerStore 里登记过的所有 trigger（cron / once / fs_watch
// / webhook）。仅读：CRUD 走 `fuxi cron *` CLI——PWA 不弹"+ 添加 trigger" 按钮，
// 因为 cron 表达式 + intent 在移动端键盘上手感差，TUI / 终端是更合适的工具层。

export const CronPage: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchCron());
  return (
    <div class={styles.page} data-testid="page-cron">
      <Show when={!data.loading} fallback={<p class={styles.empty}>加载中…</p>}>
        <Show
          when={(data()?.triggers.length ?? 0) > 0}
          fallback={<p class={styles.empty}>更漏空——还没登记任何 trigger。</p>}
        >
          <For each={data()!.triggers}>{(t) => <CronCard entry={t} />}</For>
        </Show>
      </Show>
    </div>
  );
};

const CronCard: Component<{ entry: CronEntryView }> = (props) => {
  const cls = (): string =>
    [styles.card, !props.entry.enabled ? styles.cardDisabled : ""]
      .filter(Boolean)
      .join(" ");
  return (
    <article class={cls()} data-testid={`cron-entry-${props.entry.id}`}>
      <header class={styles.head}>
        <span class={styles.intent}>{props.entry.intent || "（无 intent）"}</span>
        <span class={styles.kind}>{props.entry.kind}</span>
      </header>
      <span class={styles.summary}>{props.entry.summary}</span>
      <div class={styles.meta}>
        <Show when={props.entry.tz}>
          {(tz) => <span>tz: {tz()}</span>}
        </Show>
        <Show when={props.entry.last_fired_at}>
          {(at) => <span>上次 fire: {at()}</span>}
        </Show>
        <Show when={props.entry.consecutive_failures > 0}>
          <span class={styles.fail}>
            连失败 {props.entry.consecutive_failures}/{props.entry.max_failures}
          </span>
        </Show>
        <Show when={!props.entry.enabled}>
          <span>· 已停</span>
        </Show>
      </div>
    </article>
  );
};
