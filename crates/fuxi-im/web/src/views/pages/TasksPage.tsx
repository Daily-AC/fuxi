import {
  For,
  Show,
  createResource,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import {
  colorForTaskRole,
  formatDuration,
  formatTokens,
  shortTaskId,
} from "~/lib/format-task";
import type { TaskGroupCard, TaskMember, ToolCallSummary } from "~/types/api";
import styles from "./TasksPage.module.css";

// Page 3 · 任务树（v1 placeholder · 沿用阶段 4 扁平 members 渲染）
//
// 范围：#28 只做 shell 重构，把 TasksSheet 内容剥 BottomSheet 包装搬进 page。
// 真 C 方案两级行卡片（spec §"页 3·任务树"）由 #29 重写本组件实现。
//
// 兼容：旧 TasksSheet 的渲染路径完全保留（task-card / member / tool-call testid 不变），
// 旧测试（如有）继续 pass。

export const TasksPage: Component = () => {
  return (
    <div class={styles.page} data-testid="page-tasks">
      <header class={styles.header}>
        <h1 class={styles.title}>任务</h1>
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
          <For each={props.overview.running}>{(t) => <TaskCard task={t} />}</For>
        </section>
      </Show>
      <Show when={props.overview.completed.length > 0}>
        <section class={styles.section} data-testid="tasks-completed">
          <h3 class={styles.sectionLabel}>已完成</h3>
          <For each={props.overview.completed}>{(t) => <TaskCard task={t} dim />}</For>
        </section>
      </Show>
    </div>
  );
};

const TaskCard: Component<{ task: TaskGroupCard; dim?: boolean }> = (props) => {
  return (
    <article
      class={styles.card}
      classList={{ [styles.cardDim ?? ""]: Boolean(props.dim) }}
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
      <Show when={props.task.last_event_summary}>
        <div class={styles.summary} data-testid={`task-summary-${props.task.id}`}>
          {props.task.last_event_summary}
        </div>
      </Show>
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
    return "var(--text-muted)";
  };
  const activitySummary = (): string =>
    props.member.activity ?? props.member.last_tool_call?.tool ?? "";
  return (
    <li class={styles.memberBlock} data-testid={`member-${props.member.agent_id}`}>
      <div class={styles.memberRow}>
        <span class={styles.memberDot} style={{ background: dot() }} aria-hidden="true" />
        <span class={styles.memberName}>{props.member.role_display}</span>
        <Show when={activitySummary()}>
          <span class={`${styles.memberActivity} mono`}>{activitySummary()}</span>
        </Show>
        <Show when={props.member.tokens != null && props.member.tokens > 0}>
          <span class={`${styles.memberTokens} mono`}>{formatTokens(props.member.tokens ?? 0)}</span>
        </Show>
      </div>
      <Show when={props.member.last_tool_call}>
        <ToolCallRow call={props.member.last_tool_call!} />
      </Show>
    </li>
  );
};

const ToolCallRow: Component<{ call: ToolCallSummary }> = (props) => {
  const exit = (): string => {
    const e = props.call.exit;
    if (e === undefined || e === null) return "运行中";
    if (e === 0) return "exit 0";
    return `exit ${e}`;
  };
  const exitClass = (): string => {
    const e = props.call.exit;
    if (e === undefined || e === null) return styles.exitRunning ?? "";
    if (e === 0) return styles.exitOk ?? "";
    return styles.exitFail ?? "";
  };
  const duration = (): string => {
    if (props.call.duration_ms == null) return "";
    return formatDuration(props.call.duration_ms);
  };
  return (
    <div class={`${styles.toolCall} mono`} data-testid="member-tool-call">
      <span class={styles.toolName}>{props.call.tool}</span>
      <Show when={props.call.args_summary}>
        <span class={styles.toolArgs}> {props.call.args_summary}</span>
      </Show>
      <span class={styles.toolSep}> · </span>
      <span class={exitClass()}>{exit()}</span>
      <Show when={duration()}>
        <span class={styles.toolSep}> · </span>
        <span class={styles.toolDuration}>{duration()}</span>
      </Show>
    </div>
  );
};
