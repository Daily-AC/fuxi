import {
  For,
  Show,
  createMemo,
  createResource,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import {
  formatDuration,
  shortTaskId,
} from "~/lib/format-task";
import type { TaskGroupCard, TasksOverview } from "~/types/api";
import styles from "./TasksPage.module.css";

// 任务 tab Layer 1 · 任务列表（v3 #N3' / #38）
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §"任务 tab · Layer 1"
//
// v3 vs v2 改动：
//   - 任务卡 header tap → push Layer 2 任务 thread（不再"折叠展开 members"）
//   - members 行变 inspection-only（div，不可 tap）
//   - 删 active 高亮（per-worker 私聊概念去除）
//   - 删 "›" 推入箭头（tap 整卡进 thread，不需要 affordance）
//   - 进行中段 last_active_at 降序、已完成 sticky tail 不变

export const TasksPage: Component = () => {
  return (
    <div class={styles.page} data-testid="page-tasks">
      <header class={styles.header}>
        <h1 class={styles.title}>任务树</h1>
      </header>
      <div class={styles.body}>
        <RenderTasks />
      </div>
    </div>
  );
};

const RenderTasks: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchTasksOverview());
  // v4：卡片不再渲染门客预览，nodes 在线/离线状态搬到详情页 ⋯ 菜单。
  // TasksPage 这里去掉 fetchNodes 调用，省一个网络往返。

  return (
    <Show
      when={data()}
      fallback={
        <Show
          when={data.error}
          fallback={<p class={styles.muted} data-testid="tasks-loading">加载中…</p>}
        >
          <p class={styles.errMsg} role="alert">
            加载失败：{String(data.error)}
          </p>
        </Show>
      }
    >
      <TaskTree overview={data()!} />
    </Show>
  );
};

const TaskTree: Component<{ overview: TasksOverview }> = (props) => {
  // 进行中段按 last_active_at 降序（最近活动优先）
  const running = createMemo(() => {
    const list = [...props.overview.running];
    list.sort((a, b) => parseTs(b.last_active_at) - parseTs(a.last_active_at));
    return list;
  });
  // v3 #44 实测反馈：已完成段不再 sticky tail 折叠，直接平铺；按 last_active_at 降序（无 completed_at 字段时的最佳代理）
  const completed = createMemo(() => {
    const list = [...props.overview.completed];
    list.sort((a, b) => parseTs(b.last_active_at) - parseTs(a.last_active_at));
    return list;
  });

  const empty = (): boolean =>
    props.overview.running.length === 0 && props.overview.completed.length === 0;

  return (
    <div class={styles.root}>
      <Show when={empty()}>
        <div class={styles.emptyAll} data-testid="tasks-empty">
          <p class={styles.emptyTitle}>暂无任务</p>
          <p class={styles.emptyHint}>跟玄女说点啥，她会自动开 root task</p>
        </div>
      </Show>

      <Show when={running().length > 0}>
        <section class={styles.section} data-testid="tasks-running">
          <h3 class={styles.sectionLabel}>进行中</h3>
          <For each={running()}>{(t) => <TaskCard task={t} />}</For>
        </section>
      </Show>

      <Show when={completed().length > 0}>
        <section class={styles.section} data-testid="tasks-completed">
          <h3 class={styles.sectionLabel}>已完成</h3>
          <For each={completed()}>{(t) => <TaskCard task={t} dim />}</For>
        </section>
      </Show>
    </div>
  );
};

function parseTs(iso: string): number {
  const t = new Date(iso).getTime();
  return Number.isNaN(t) ? 0 : t;
}

const TaskCard: Component<{ task: TaskGroupCard; dim?: boolean }> = (props) => {
  const { navPush } = useApi();
  const memberCount = (): number => props.task.members.length;

  // 卡片整体一个 button — 移除底部门客预览块（用户实测反馈：误点击 +
  // role 兜底 unknown 体验差）。门客详情移到详情页右上 ⋯ 菜单（TaskThreadPage）。
  const onCardTap = (): void => {
    navPush({
      kind: "task",
      task_id: props.task.id,
      title: props.task.title,
    });
  };

  return (
    <article
      class={styles.card}
      classList={{ [styles.cardDim ?? ""]: Boolean(props.dim) }}
      data-testid={`task-card-${props.task.id}`}
      data-status={props.task.status}
    >
      <button
        type="button"
        class={styles.cardHead}
        onClick={onCardTap}
        data-testid={`task-card-head-${props.task.id}`}
        aria-label={`进入任务 ${props.task.title}`}
      >
        <span class={styles.cardId}>
          <span class="agent-id">{shortTaskId(props.task.id)}</span>
          <span class={styles.cardTitle}>{props.task.title}</span>
        </span>
        <span class={styles.cardMeta}>
          <Show when={memberCount() > 0}>
            <span class={styles.memberCount}>{memberCount()} 门客</span>
            <span class={styles.metaSep} aria-hidden="true">·</span>
          </Show>
          <time class={styles.duration}>{formatDuration(props.task.duration_ms)}</time>
        </span>
      </button>
    </article>
  );
};
