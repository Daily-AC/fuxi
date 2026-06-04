import {
  For,
  Show,
  createMemo,
  createResource,
  type Component,
  type JSX,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { EmptyState } from "~/components/ui/EmptyState";
import { SectionLabel } from "~/components/ui/SectionLabel";
import { Skeleton } from "~/components/ui/Skeleton";
import { StatePill, type StatePillTone } from "~/components/ui/StatePill";
import { formatDuration, shortTaskId } from "~/lib/format-task";
import type { TaskGroupCard, TaskMember, TasksOverview } from "~/types/api";
import styles from "./TasksPage.module.css";

// 任务 tab Layer 1 · 任务列表 · daimeng 奶油糖果重构（archetype A · 列表/收件箱）。
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §"任务 tab · Layer 1"
//
// RESKIN：保留全部行为 + data-testid（page-tasks / task-card-<id> /
// task-card-head-<id> / tasks-running / tasks-completed / tasks-empty）。
// 视觉换成共享原语：行卡 (ListRow look) + StatePill 状态 + SectionLabel 分区 +
// EmptyState 空态。页底 u-mesh 暖光网格。

// 任务清单图标（inline SVG，禁 emoji）。色由 iconSlot 按 tone 染。
const ChecklistIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M10 6h9M10 12h9M10 18h9" stroke-linecap="round" />
    <path
      d="m4 5.5 1.4 1.4L8 4M4 11.5l1.4 1.4L8 10M4 17.5l1.4 1.4L8 16"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
  </svg>
);

export const TasksPage: Component = () => {
  return (
    <div class={`u-mesh u-noise ${styles.page}`} data-testid="page-tasks">
      <header class={styles.header}>
        <h1 class={`u-title ${styles.title}`}>任务</h1>
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

  return (
    <Show
      when={data()}
      fallback={
        <Show
          when={data.error}
          fallback={
            <div class={styles.skeletons} data-testid="tasks-loading">
              <Skeleton height="60px" />
              <Skeleton height="60px" />
              <Skeleton height="60px" />
            </div>
          }
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
  // 已完成段直接平铺，按 last_active_at 降序（无 completed_at 字段时的最佳代理）
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
        <div data-testid="tasks-empty">
          <EmptyState
            title="还没有任务～"
            hint="跟玄女说点啥，她会自动开 root task"
            mascotState="sleep"
          />
        </div>
      </Show>

      <Show when={running().length > 0}>
        <section class={styles.section} data-testid="tasks-running">
          <SectionLabel>进行中</SectionLabel>
          <div class={styles.list}>
            <For each={running()}>{(t) => <TaskCard task={t} />}</For>
          </div>
        </section>
      </Show>

      <Show when={completed().length > 0}>
        <section class={styles.section} data-testid="tasks-completed">
          <SectionLabel>已完成</SectionLabel>
          <div class={styles.list}>
            <For each={completed()}>{(t) => <TaskCard task={t} dim />}</For>
          </div>
        </section>
      </Show>
    </div>
  );
};

function parseTs(iso: string): number {
  const t = new Date(iso).getTime();
  return Number.isNaN(t) ? 0 : t;
}

// status → StatePill tone（running 桃 / completed 薄荷 / 其余 queued 薰衣草）。
function pillFor(status: string): { label: string; tone: StatePillTone } {
  switch (status) {
    case "running":
      return { label: "进行中", tone: "running" };
    case "completed":
      return { label: "已完成", tone: "done" };
    default:
      return { label: "排队中", tone: "queued" };
  }
}

// 卡片 iconSlot 色调：按首个门客角色染（鲁班 peach / 蒲松 mint / 玄女 lavender）。
function toneForRole(role: string | undefined): string {
  switch (role) {
    case "luban":
    case "鲁班":
      return "plain"; // peach 默认 iconSlot 即暖橙
    case "pusong":
    case "蒲松":
      return "mint";
    case "xuannv":
    case "玄女":
      return "lavender";
    default:
      return "plain";
  }
}

// 副标题：门客活动摘要。有门客 → "鲁班 · grep server/api.go"；无 → 时长 only。
function memberSummary(members: TaskMember[]): string {
  const lead = members[0];
  if (!lead) return "";
  const display = lead.role_display || lead.role;
  const tool =
    typeof lead.last_tool_call === "string"
      ? lead.last_tool_call
      : (lead.last_tool_call?.tool ?? lead.activity ?? "");
  return tool ? `${display} · ${tool}` : display;
}

const TaskCard: Component<{ task: TaskGroupCard; dim?: boolean }> = (props) => {
  const { navPush } = useApi();
  const memberCount = (): number => props.task.members.length;

  // 整卡 tap → push Layer 2 任务 thread（门客详情移到详情页右上 ⋯ 菜单）。
  const onCardTap = (): void => {
    navPush({
      kind: "task",
      task_id: props.task.id,
      title: props.task.title,
    });
  };

  const pill = (): { label: string; tone: StatePillTone } =>
    pillFor(props.task.status);
  const subtitle = (): string => memberSummary(props.task.members);

  return (
    <article
      class={styles.card}
      classList={{ [styles.cardDim ?? ""]: Boolean(props.dim) }}
      data-testid={`task-card-${props.task.id}`}
      data-status={props.task.status}
    >
      <button
        type="button"
        class={`u-card ${styles.row}`}
        onClick={onCardTap}
        data-testid={`task-card-head-${props.task.id}`}
        aria-label={`进入任务 ${props.task.title}`}
      >
        <span
          class={styles.iconSlot}
          data-tone={toneForRole(props.task.members[0]?.role)}
          aria-hidden="true"
        >
          <ChecklistIcon />
        </span>
        <span class={styles.mid}>
          <span class={styles.titleLine}>
            <span class={`agent-id ${styles.cardId}`}>
              {shortTaskId(props.task.id)}
            </span>
            <span class={styles.cardTitle}>{props.task.title}</span>
          </span>
          <span class={styles.sub}>
            <Show
              when={subtitle()}
              fallback={
                <span class={styles.subDim}>
                  <Show when={memberCount() > 0}>
                    <span class={styles.memberCount}>{memberCount()} 门客</span>
                    <span class={styles.metaSep} aria-hidden="true">
                      ·
                    </span>
                  </Show>
                  <time class={styles.duration}>
                    {formatDuration(props.task.duration_ms)}
                  </time>
                </span>
              }
            >
              <span class={styles.subText}>{subtitle()}</span>
              <span class={styles.metaTail}>
                <Show when={memberCount() > 0}>
                  <span class={styles.memberCount}>{memberCount()} 门客</span>
                  <span class={styles.metaSep} aria-hidden="true">
                    ·
                  </span>
                </Show>
                <time class={styles.duration}>
                  {formatDuration(props.task.duration_ms)}
                </time>
              </span>
            </Show>
          </span>
        </span>
        <span class={styles.trail}>
          <StatePill label={pill().label} tone={pill().tone} />
        </span>
      </button>
    </article>
  );
};
