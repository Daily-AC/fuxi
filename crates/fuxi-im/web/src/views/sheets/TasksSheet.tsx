import {
  For,
  Show,
  createResource,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { BottomSheet } from "~/components/BottomSheet";
import {
  colorForTaskRole,
  formatDuration,
  formatTokens,
  shortTaskId,
} from "~/lib/format-task";
import type { TaskGroupCard, TaskMember } from "~/types/api";
import styles from "./TasksSheet.module.css";

// 任务 sheet · header「任务」+ body 分组「进行中」/「已完成」。
// open=false 时不渲染（BottomSheet 内已处理），所以 createResource 仅在 open 时跑——
// 通过把 createResource 放在 RenderTasks 子组件里实现。
export const TasksSheet: Component = () => {
  const { activeSheet, setActiveSheet } = useApi();
  const open = (): boolean => activeSheet() === "tasks";

  return (
    <BottomSheet
      open={open()}
      onClose={() => setActiveSheet(null)}
      title="任务"
      testId="tasks-sheet"
    >
      <Show when={open()}>
        <RenderTasks />
      </Show>
    </BottomSheet>
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
      <TaskGroups overview={data()!} />
    </Show>
  );
};

const TaskGroups: Component<{ overview: { running: TaskGroupCard[]; completed: TaskGroupCard[] } }> = (
  props,
) => {
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
      <Show when={props.overview.running.length > 0}>
        <section class={styles.section} data-testid="tasks-running">
          <h3 class={styles.sectionLabel}>进行中</h3>
          <For each={props.overview.running}>{(t) => <RunningCard task={t} />}</For>
        </section>
      </Show>
      <Show when={props.overview.completed.length > 0}>
        <section class={styles.section} data-testid="tasks-completed">
          <h3 class={styles.sectionLabel}>已完成</h3>
          <For each={props.overview.completed}>{(t) => <CompletedRow task={t} />}</For>
        </section>
      </Show>
    </div>
  );
};

const RunningCard: Component<{ task: TaskGroupCard }> = (props) => {
  return (
    <article
      class={styles.card}
      data-testid={`task-card-${props.task.id}`}
      data-status={props.task.status}
    >
      <header class={styles.cardHead}>
        <span class={styles.cardId}>
          <span class="agent-id">{shortTaskId(props.task.id)}</span>{" "}
          <span class={styles.cardTitle}>{props.task.title}</span>
        </span>
        <time class={styles.duration}>{formatDuration(props.task.duration_ms)}</time>
      </header>
      <Show when={props.task.members.length > 0}>
        <ul class={styles.members}>
          <For each={props.task.members}>{(m) => <MemberRow member={m} />}</For>
        </ul>
      </Show>
    </article>
  );
};

const MemberRow: Component<{ member: TaskMember }> = (props) => {
  const dot = (): string => {
    if (props.member.status === "busy") return colorForTaskRole(props.member.role);
    if (props.member.status === "thinking") return "var(--accent)";
    return "var(--text-muted)"; // idle
  };
  return (
    <li class={styles.memberRow} data-testid={`member-${props.member.agent_id}`}>
      <span class={styles.memberDot} style={{ background: dot() }} aria-hidden="true" />
      <span class={styles.memberName}>{props.member.role_display}</span>
      <Show when={props.member.activity}>
        <span class={`${styles.memberActivity} mono`}>{props.member.activity}</span>
      </Show>
      <Show when={props.member.tokens != null && props.member.tokens > 0}>
        <span class={`${styles.memberTokens} mono`}>{formatTokens(props.member.tokens ?? 0)}</span>
      </Show>
    </li>
  );
};

const CompletedRow: Component<{ task: TaskGroupCard }> = (props) => {
  return (
    <div
      class={styles.completedRow}
      data-testid={`task-completed-${props.task.id}`}
      data-status={props.task.status}
    >
      <span class={styles.completedHead}>
        <span class="agent-id">{shortTaskId(props.task.id)}</span>{" "}
        <span class={styles.completedTitle}>{props.task.title}</span>
      </span>
      <span class={styles.completedDuration}>{formatDuration(props.task.duration_ms)}</span>
    </div>
  );
};
