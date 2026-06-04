import {
  For,
  Show,
  createMemo,
  createResource,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { Card } from "~/components/ui/Card";
import { ScreenHeader } from "~/components/ui/ScreenHeader";
import { SectionLabel } from "~/components/ui/SectionLabel";
import { StatePill } from "~/components/ui/StatePill";
import type {
  DeliverableEntry,
  EphemeralActiveView,
  EphemeralArchivedView,
  ProjectView,
  SandboxView,
} from "~/types/api";
import styles from "./ProjectDetailPage.module.css";

// 项目详情 Layer 2 · Decision 21 phase 3 · daimeng 奶油糖果重绘（archetype B · 详情）。
//
// 数据源：
//   GET /api/projects                      → 找到当前 project view
//   GET /api/projects/{id}/sandboxes       → L3 持久 sandbox 列表
//   GET /api/projects/{id}/ephemeral       → L2 active / archived
//   GET /api/deliverables                  → 该 project 的所有交付（前端 filter）
//
// 交互：ScreenHeader 返回 → navPop()。
//   summary Card（路径 + 默认分支 chip）；SectionLabel 分组 sandbox / L2 / 交付 各 Card 行。
//   点交付卡 → navTo 跨 tab 跳「交付」详情。
//
// RESKIN：保留全部 data-testid（page-project-detail / project-detail-back /
// project-detail-loading / project-detail-meta / pdetail-sandbox-<id> /
// pdetail-l2-active-<id> / pdetail-l2-archived-<id> / pdetail-deliverable-<task> /
// pdetail-deliverable-open-<task>）+ 行为。视觉换共享原语，去暗色。

export interface ProjectDetailPageProps {
  project_id: string;
}

export const ProjectDetailPage: Component<ProjectDetailPageProps> = (props) => {
  const { client, navPop } = useApi();
  const [projects] = createResource(() => client.fetchProjects());
  const project = createMemo<ProjectView | null>(() => {
    const list = projects()?.projects ?? [];
    return list.find((p) => p.id === props.project_id) ?? null;
  });

  const [sandboxes] = createResource(() => client.fetchSandboxes(props.project_id));
  const [ephemeral] = createResource(() => client.fetchEphemeral(props.project_id));
  const [deliverables] = createResource(() => client.fetchDeliverables());
  const myDeliverables = createMemo<DeliverableEntry[]>(() => {
    const all = deliverables()?.deliverables ?? [];
    return all.filter((e) => e.project === props.project_id);
  });

  return (
    <div class={`u-mesh u-noise ${styles.page}`} data-testid="page-project-detail">
      <ScreenHeader title={props.project_id} onBack={() => navPop()} />
      <div class={styles.body}>
        <Show
          when={project()}
          fallback={
            <p class={styles.muted} data-testid="project-detail-loading">
              加载中…
            </p>
          }
        >
          <Card class={styles.summary} tone="peach">
            <div class={styles.metaGrid} data-testid="project-detail-meta">
              <div class={styles.metaRow}>
                <span class={styles.metaLabel}>项目路径</span>
                <span class={`${styles.metaValue} mono`} title={project()!.canonical_path}>
                  {project()!.canonical_path}
                </span>
              </div>
              <div class={styles.metaRow}>
                <span class={styles.metaLabel}>默认分支</span>
                <span class={styles.metaChip}>
                  <StatePill label={project()!.default_branch} tone="queued" />
                </span>
              </div>
            </div>
          </Card>

          <section class={styles.section}>
            <SectionLabel>L3 持久 sandbox</SectionLabel>
            <Show
              when={sandboxes()}
              fallback={<p class={styles.muted}>加载中…</p>}
            >
              <Show
                when={sandboxes()!.sandboxes.length > 0}
                fallback={
                  <Card class={styles.emptyCard}>
                    <p class={styles.empty}>
                      暂无 sandbox · 在 home 上跑{" "}
                      <span class="mono">
                        fuxi spawn --role &lt;role&gt; --project {props.project_id}
                      </span>
                    </p>
                  </Card>
                }
              >
                <ul class={styles.list}>
                  <For each={sandboxes()!.sandboxes}>
                    {(s) => <SandboxRow sandbox={s} />}
                  </For>
                </ul>
              </Show>
            </Show>
          </section>

          <section class={styles.section}>
            <SectionLabel>L2 一次性工作区</SectionLabel>
            <Show
              when={ephemeral()}
              fallback={<p class={styles.muted}>加载中…</p>}
            >
              <h3 class={styles.subTitle}>Active</h3>
              <Show
                when={ephemeral()!.active.length > 0}
                fallback={
                  <Card class={styles.emptyCard}>
                    <p class={styles.empty}>无活跃 worktree</p>
                  </Card>
                }
              >
                <ul class={styles.list}>
                  <For each={ephemeral()!.active}>
                    {(a) => <ActiveRow active={a} />}
                  </For>
                </ul>
              </Show>
              <h3 class={styles.subTitle}>归档（24h 内）</h3>
              <Show
                when={ephemeral()!.archived.length > 0}
                fallback={
                  <Card class={styles.emptyCard}>
                    <p class={styles.empty}>无归档</p>
                  </Card>
                }
              >
                <ul class={styles.list}>
                  <For each={ephemeral()!.archived}>
                    {(a) => <ArchivedRow archived={a} />}
                  </For>
                </ul>
              </Show>
            </Show>
          </section>

          <section class={styles.section}>
            <SectionLabel>交付</SectionLabel>
            <Show
              when={deliverables()}
              fallback={<p class={styles.muted}>加载中…</p>}
            >
              <Show
                when={myDeliverables().length > 0}
                fallback={
                  <Card class={styles.emptyCard}>
                    <p class={styles.empty}>无交付</p>
                  </Card>
                }
              >
                <ul class={styles.list}>
                  <For each={myDeliverables()}>
                    {(d) => <DeliverableSummaryRow entry={d} />}
                  </For>
                </ul>
              </Show>
            </Show>
          </section>
        </Show>
      </div>
    </div>
  );
};

const SandboxRow: Component<{ sandbox: SandboxView }> = (props) => (
  <li
    class={`u-card ${styles.row}`}
    data-testid={`pdetail-sandbox-${props.sandbox.workspace_id}`}
  >
    <div class={styles.rowHead}>
      <span class={styles.rowLabel}>{props.sandbox.role}</span>
      <span class={`${styles.rowSub} mono`}>{props.sandbox.branch}</span>
    </div>
    <span class={`${styles.rowPath} mono`} title={props.sandbox.path}>
      {props.sandbox.path}
    </span>
  </li>
);

const ActiveRow: Component<{ active: EphemeralActiveView }> = (props) => (
  <li
    class={`u-card ${styles.row}`}
    data-testid={`pdetail-l2-active-${props.active.workspace_id}`}
  >
    <div class={styles.rowHead}>
      <span class={`${styles.rowLabel} mono`}>{props.active.task}</span>
      <span class={`${styles.rowSub} mono`}>{props.active.branch}</span>
    </div>
    <span class={`${styles.rowPath} mono`} title={props.active.path}>
      {props.active.path}
    </span>
  </li>
);

const ArchivedRow: Component<{ archived: EphemeralArchivedView }> = (props) => {
  const archivedAt = (): string => {
    const t = new Date(props.archived.archived_at);
    if (Number.isNaN(t.getTime())) return props.archived.archived_at;
    return t.toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  };
  return (
    <li
      class={`u-card ${styles.row}`}
      data-testid={`pdetail-l2-archived-${props.archived.workspace_id}`}
    >
      <div class={styles.rowHead}>
        <span class={`${styles.rowLabel} mono`}>{props.archived.task}</span>
        <span class={`${styles.rowSub} mono`}>{props.archived.branch}</span>
      </div>
      <time class={styles.rowSub}>归档于 {archivedAt()}</time>
    </li>
  );
};

const DeliverableSummaryRow: Component<{ entry: DeliverableEntry }> = (props) => {
  const { navTo } = useApi();
  const taskShort = (): string =>
    props.entry.task.length > 8 ? props.entry.task.slice(0, 8) : props.entry.task;
  const fileCount = (): number => props.entry.files.length;
  const open = (): void => {
    // 跨 tab 跳到「交付」详情——v1-session17 task #9 后 navTo 原子化
    // setActiveTab(3 更多) + setMoreSub("deliverables") + navPush(deliverable)。
    navTo({
      kind: "deliverable",
      project_id: props.entry.project,
      task_id: props.entry.task,
    });
  };
  return (
    <li
      class={`u-card ${styles.row} ${styles.rowTappable}`}
      data-testid={`pdetail-deliverable-${props.entry.task}`}
    >
      <button
        type="button"
        class={styles.rowBtn}
        onClick={() => open()}
        data-testid={`pdetail-deliverable-open-${props.entry.task}`}
        aria-label={`查看交付 ${props.entry.kind} task-${taskShort()}`}
      >
        <span class={styles.rowMid}>
          <span class={styles.rowLabel}>{props.entry.kind}</span>
          <span class={styles.rowSubLine}>
            <span class={`${styles.rowSub} mono`}>task-{taskShort()}</span>
            <span class={styles.rowSep} aria-hidden="true">·</span>
            <span class={styles.rowSub}>
              {fileCount()} 文件 · {props.entry.status}
            </span>
          </span>
        </span>
        <span class={styles.chevron} aria-hidden="true">
          <svg width="8" height="14" viewBox="0 0 8 14" fill="none">
            <path
              d="M1.5 1 6.5 7l-5 6"
              stroke="currentColor"
              stroke-width="1.8"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </span>
      </button>
    </li>
  );
};
