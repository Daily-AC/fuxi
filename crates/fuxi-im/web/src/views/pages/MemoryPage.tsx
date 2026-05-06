import { For, Show, createResource, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import type { MemoryFactView } from "~/types/api";
import styles from "./MemoryPage.module.css";

// 「更多 → 记忆」· v1-session17 task #9
//
// 把策府（OracleStore）现行事实按 subject 分组列出来。仅读，不改——编辑由
// 玄女自己在对话里 supersede / fuxi-cli `oracle` 子命令。

export const MemoryPage: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchMemory({ limit: 200 }));

  return (
    <div class={styles.page} data-testid="page-memory">
      <Show when={!data.loading} fallback={<p class={styles.empty}>加载中…</p>}>
        <Show
          when={(data()?.total ?? 0) > 0}
          fallback={<p class={styles.empty}>策府目前空着——玄女还没记下任何事实。</p>}
        >
          <p class={styles.summary} data-testid="memory-summary">
            策府 共 {data()!.total} 条事实，{data()!.groups.length} 个 subject。
          </p>
          <For each={data()!.groups}>
            {(g) => (
              <section class={styles.group} data-testid={`memory-group-${g.subject}`}>
                <header class={styles.groupHead}>
                  <span class={styles.subject}>{g.subject}</span>
                  <span class={styles.count}>{g.facts.length} 条</span>
                </header>
                <For each={g.facts}>{(f) => <FactCard fact={f} />}</For>
              </section>
            )}
          </For>
        </Show>
      </Show>
    </div>
  );
};

const FactCard: Component<{ fact: MemoryFactView }> = (props) => {
  const conf = (): string => {
    const c = props.fact.confidence;
    return c >= 0.8 ? styles.confHigh! : styles.confLow!;
  };
  return (
    <article class={styles.fact} data-testid={`memory-fact-${props.fact.id}`}>
      <span class={styles.predicate}>{props.fact.predicate}</span>
      <span class={styles.object}>{props.fact.object}</span>
      <div class={styles.meta}>
        <span class={conf()}>置信 {(props.fact.confidence * 100).toFixed(0)}%</span>
        <span>· {props.fact.source}</span>
      </div>
    </article>
  );
};
