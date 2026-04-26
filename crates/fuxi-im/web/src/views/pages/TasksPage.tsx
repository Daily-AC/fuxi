import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import {
  formatDuration,
  shortTaskId,
} from "~/lib/format-task";
import type { TaskGroupCard, TaskMember, TasksOverview } from "~/types/api";
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
  const [completedExpanded, setCompletedExpanded] = createSignal(false);

  // 进行中段按 last_active_at 降序（最近活动优先）
  const running = createMemo(() => {
    const list = [...props.overview.running];
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

      <Show when={props.overview.completed.length > 0}>
        <section class={styles.section} data-testid="tasks-completed">
          <Show when={!completedExpanded()}>
            <button
              type="button"
              class={styles.completedTail}
              onClick={() => setCompletedExpanded(true)}
              data-testid="tasks-completed-tail"
              aria-expanded={false}
            >
              <span>已完成 · {props.overview.completed.length} 条</span>
              <span class={styles.tailMark} aria-hidden="true">▸</span>
            </button>
          </Show>
          <Show when={completedExpanded()}>
            <h3 class={styles.sectionLabel}>已完成</h3>
            <For each={props.overview.completed}>{(t) => <TaskCard task={t} dim />}</For>
          </Show>
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

  // v3：整卡 button，tap → push Layer 2 任务 thread
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
      <Show when={memberCount() > 0}>
        <ul class={styles.members}>
          <For each={props.task.members}>{(m) => <MemberRow member={m} />}</For>
        </ul>
      </Show>
    </article>
  );
};

// v3：member 行变 inspection-only（div 不再 button）。
// 数据：role · last_tool_call/activity 副文本（同 v2 副文本逻辑）。
const MemberRow: Component<{ member: TaskMember }> = (props) => {
  const sub = (): string => {
    const tool = props.member.last_tool_call?.tool;
    if (tool) {
      const args = props.member.last_tool_call?.args_summary;
      return args ? `${tool} ${args}` : tool;
    }
    if (props.member.activity) return props.member.activity;
    if (props.member.status === "idle") return "待命";
    if (props.member.status === "thinking") return "思考中";
    return "运行中";
  };

  return (
    <li
      class={styles.memberRow}
      data-testid={`member-${props.member.agent_id}`}
    >
      <div class={styles.memberMain}>
        <span class={styles.memberRole}>{props.member.role_display}</span>
        <span class={`${styles.memberSub} mono`}>{sub()}</span>
      </div>
    </li>
  );
};
