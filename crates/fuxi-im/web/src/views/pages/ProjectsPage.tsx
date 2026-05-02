import { For, Show, createResource, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import type { ProjectView } from "~/types/api";
import styles from "./ProjectsPage.module.css";

// 项目 tab · Decision 21 phase 1
//
// PWA 视角的"工作区"门面——展示已注册 project 的 canonical 路径 + 默认分支。
// v1 只读：注册 / 删除走 CLI（fuxi project add / rm）；后续若加 GUI 注册流
// 在这里加按钮。
//
// 数据源：GET /api/projects → ProjectsResponse{ projects: [...] }
// 503 路径：registry 未注入（部署侧 $HOME 没设）；前端显示空态 + 提示。

export const ProjectsPage: Component = () => {
  return (
    <div class={styles.page} data-testid="page-projects">
      <header class={styles.header}>
        <h1 class={styles.title}>项目</h1>
      </header>
      <div class={styles.body}>
        <RenderProjects />
      </div>
    </div>
  );
};

const RenderProjects: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchProjects());

  return (
    <Show
      when={data()}
      fallback={
        <Show
          when={data.error}
          fallback={
            <p class={styles.muted} data-testid="projects-loading">
              加载中…
            </p>
          }
        >
          <p class={styles.errMsg} role="alert">
            加载失败：{String(data.error)}
          </p>
        </Show>
      }
    >
      <Show
        when={data()!.projects.length > 0}
        fallback={
          <div class={styles.empty} data-testid="projects-empty">
            <p class={styles.emptyTitle}>暂无项目</p>
            <p class={styles.emptyHint}>
              在 home 上跑 <span class="mono">fuxi project add ~/your-repo</span> 注册
            </p>
          </div>
        }
      >
        <ul class={styles.list}>
          <For each={data()!.projects}>{(p) => <ProjectCard project={p} />}</For>
        </ul>
      </Show>
    </Show>
  );
};

const ProjectCard: Component<{ project: ProjectView }> = (props) => {
  const created = (): string => {
    const t = new Date(props.project.created_at);
    if (Number.isNaN(t.getTime())) return props.project.created_at;
    return t.toLocaleDateString("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
  };
  return (
    <li
      class={styles.card}
      data-testid={`project-card-${props.project.id}`}
    >
      <div class={styles.cardHead}>
        <span class={styles.cardId}>{props.project.id}</span>
        <span class={styles.cardBranch}>
          <span class={styles.branchLabel}>默认分支</span>
          <span class={`${styles.branchValue} mono`}>
            {props.project.default_branch}
          </span>
        </span>
      </div>
      <div class={`${styles.cardPath} mono`} title={props.project.canonical_path}>
        {props.project.canonical_path}
      </div>
      <div class={styles.cardMeta}>
        <time class={styles.cardCreated}>注册于 {created()}</time>
      </div>
    </li>
  );
};
