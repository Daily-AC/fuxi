import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { useApi, type NavRoute } from "~/components/ApiProvider";
import {
  formatDuration,
  shortTaskId,
} from "~/lib/format-task";
import type { TaskGroupCard, TaskMember, TasksOverview } from "~/types/api";
import styles from "./TasksPage.module.css";

// Page 3 · 任务树 · C 方案两级行卡片（#N2 / #29）
// 设计 spec: docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §"页 3·任务树"
//
// 布局：
//   进行中段（卡片 header 含 title/elapsed/门客数 + members 行（每行 ≥ 32px，role 加粗 + tool 副文本 + › 箭头））
//   已完成段默认折叠 sticky tail "已完成 · N 条 ▸"
// 交互：
//   任务 header tap → 折叠/展开 members（视觉态，不切 active）
//   member 整行 (含 ›) tap → navPush({ kind: "worker", agent_id, role_display })
//   已完成 sticky tail tap → 展开已完成段
// active 高亮：navRoute.agent_id 匹配的 member 行加左 2px 橙边 + 浅橙背景

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
  // 默认展开 members（用户可 tap header 收起）；折叠是视觉态，不影响 active
  const [expanded, setExpanded] = createSignal(true);
  const memberCount = (): number => props.task.members.length;

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
        onClick={() => setExpanded(!expanded())}
        aria-expanded={expanded()}
        data-testid={`task-card-head-${props.task.id}`}
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
      <Show when={expanded() && memberCount() > 0}>
        <ul class={styles.members}>
          <For each={props.task.members}>{(m) => <MemberRow member={m} />}</For>
        </ul>
      </Show>
    </article>
  );
};

const MemberRow: Component<{ member: TaskMember }> = (props) => {
  const { navRoute, navPush } = useApi();

  const isActive = (): boolean => {
    const r = navRoute();
    return r?.kind === "worker" && r.agent_id === props.member.agent_id;
  };

  const sub = (): string => {
    // 副文本优先 last_tool_call.tool（精准），fallback activity（旧字段），fallback 状态
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

  const onTap = (): void => {
    const route: NonNullable<NavRoute> = {
      kind: "worker",
      agent_id: props.member.agent_id,
      role_display: props.member.role_display,
    };
    navPush(route);
  };

  return (
    <li
      class={styles.memberRow}
      classList={{ [styles.memberRowActive ?? ""]: isActive() }}
      data-testid={`member-${props.member.agent_id}`}
      data-active={isActive() ? "true" : undefined}
    >
      <button
        type="button"
        class={styles.memberBtn}
        onClick={onTap}
        aria-label={`私聊 ${props.member.role_display}`}
      >
        <div class={styles.memberMain}>
          <span class={styles.memberRole}>{props.member.role_display}</span>
          <span class={`${styles.memberSub} mono`}>{sub()}</span>
        </div>
        <span class={styles.chev} aria-hidden="true">›</span>
      </button>
    </li>
  );
};
