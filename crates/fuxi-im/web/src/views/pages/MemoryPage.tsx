import { For, Show, createMemo, createResource, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import type {
  HetuPatternView,
  MemoryFactView,
  UserProfileView,
} from "~/types/api";
import styles from "./MemoryPage.module.css";

// 「更多 → 记忆」· v1-session19 升级为 3 层视图：
//
//   1. 身份 (user_profile) ·    fuxi profile set / 仓颉抽——身份卡 / 偏好 / tone
//   2. 事实 (oracle_facts) ·    玄女策府现行事实（v1-session19 #3 起过滤掉
//                                session_id / worktree 这类基础设施 predicate）
//   3. 经验 (hetu_patterns) ·   河图洛书——门客经验 + insight，可迁移模式
//
// 三层并存：用户实测 home 上 oracle 全是 infra 噪音被过滤后页面看着空，但 hetu
// 那边已经积了 28 条真知识——v1-session19 #2 把 hetu / user_profile 接进来。

export const MemoryPage: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchMemory({ limit: 200 }));

  const profile = createMemo<UserProfileView[]>(() => data()?.user_profile ?? []);
  const insights = createMemo<HetuPatternView[]>(() => data()?.insights ?? []);
  const groups = createMemo(() => data()?.groups ?? []);

  const factsTotal = createMemo<number>(() => data()?.total ?? 0);
  const isEmpty = createMemo<boolean>(
    () =>
      profile().length === 0 && insights().length === 0 && factsTotal() === 0,
  );

  return (
    <div class={styles.page} data-testid="page-memory">
      <Show when={!data.loading} fallback={<p class={styles.empty}>加载中…</p>}>
        <Show
          when={!isEmpty()}
          fallback={
            <p class={styles.empty}>
              记忆目前空着——先 <code>fuxi profile set</code> 写身份卡，或派门客
              干活让仓颉自动抽 insight。
            </p>
          }
        >
          {/* 身份卡 section（识别用户身份 / 偏好 / tone） */}
          <Show when={profile().length > 0}>
            <section class={styles.layer} data-testid="memory-section-profile">
              <header class={styles.layerHead}>
                <span class={styles.layerName}>身份卡</span>
                <span class={styles.layerSub}>
                  spawn 时自动注入门客 prompt · {profile().length} 条
                </span>
              </header>
              <For each={profile()}>{(p) => <ProfileCard profile={p} />}</For>
            </section>
          </Show>

          {/* 事实策府 section（玄女现行事实，过 infra predicate） */}
          <Show when={factsTotal() > 0}>
            <section class={styles.layer} data-testid="memory-section-facts">
              <header class={styles.layerHead}>
                <span class={styles.layerName}>事实策府</span>
                <span class={styles.layerSub}>
                  共 {factsTotal()} 条 · {groups().length} 个 subject
                </span>
              </header>
              <For each={groups()}>
                {(g) => (
                  <div class={styles.group} data-testid={`memory-group-${g.subject}`}>
                    <header class={styles.groupHead}>
                      <span class={styles.subject}>{g.subject}</span>
                      <span class={styles.count}>{g.facts.length} 条</span>
                    </header>
                    <For each={g.facts}>{(f) => <FactCard fact={f} />}</For>
                  </div>
                )}
              </For>
            </section>
          </Show>

          {/* 河图洛书 section（仓颉抽的 insight） */}
          <Show when={insights().length > 0}>
            <section class={styles.layer} data-testid="memory-section-insights">
              <header class={styles.layerHead}>
                <span class={styles.layerName}>河图洛书</span>
                <span class={styles.layerSub}>
                  仓颉抽出的可迁移经验 · {insights().length} 条
                </span>
              </header>
              <For each={insights()}>{(i) => <InsightCard insight={i} />}</For>
            </section>
          </Show>
        </Show>
      </Show>
    </div>
  );
};

const ProfileCard: Component<{ profile: UserProfileView }> = (props) => (
  <article class={styles.fact} data-testid={`memory-profile-${props.profile.id}`}>
    <span class={styles.predicate}>{props.profile.key}</span>
    <span class={styles.object}>{props.profile.value}</span>
    <div class={styles.meta}>
      <span>· 来源 {props.profile.source}</span>
    </div>
  </article>
);

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

const InsightCard: Component<{ insight: HetuPatternView }> = (props) => {
  const conf = (): string => {
    return props.insight.confidence >= 0.8 ? styles.confHigh! : styles.confLow!;
  };
  return (
    <article
      class={styles.fact}
      data-testid={`memory-insight-${props.insight.id}`}
    >
      <span class={styles.predicate}>
        {props.insight.role} · {props.insight.task_type}
      </span>
      <span class={styles.object}>{props.insight.pattern}</span>
      <div class={styles.meta}>
        <span class={conf()}>
          置信 {(props.insight.confidence * 100).toFixed(0)}%
        </span>
      </div>
    </article>
  );
};
