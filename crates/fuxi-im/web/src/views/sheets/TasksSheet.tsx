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
import type { TaskGroupCard, TaskMember, ToolCallSummary } from "~/types/api";
import styles from "./TasksSheet.module.css";

// 任务 sheet · header「任务」+ body 分组「进行中」/「已完成」。
// #26：completed 卡也全显 members + last_event_summary + member last_tool_call 详情，
// 信息密度对齐 TUI 任务树。树状缩进用 CSS padding + border-left 模拟，不引入 ┌─└ unicode。
//
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

// 单 task 卡片 · 进行中 / 已完成共用，dim 时压暗一档。
// 排版：
//   顶行：[#id 16px · title 13px secondary · ... · duration 13px muted]
//   summary 行（如有 last_event_summary）：13px muted，padding-left + 左 1px border 模拟树状
//   members 列表：每 member 一行 grid + 可选下方 toolcall 二级行（padding-left + border-left 缩进）
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
    return "var(--text-muted)"; // idle
  };
  // activity 优先取 last_tool_call.tool（更精准），fallback 到 activity 字段
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
  // 二级行 · 树状缩进用 padding-left + 1px border-left 实现，不引入 ┌─└ unicode 装饰。
  // 内容：mono · tool · args · exit 码 · 时长
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
