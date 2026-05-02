import {
  For,
  Show,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { ApiError } from "~/lib/api";
import { useApi } from "~/components/ApiProvider";
import type { AddProjectRequest, ProjectView } from "~/types/api";
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
  const { client } = useApi();
  const [data, { refetch }] = createResource(() => client.fetchProjects());
  const [addOpen, setAddOpen] = createSignal(false);

  return (
    <div class={styles.page} data-testid="page-projects">
      <header class={styles.header}>
        <h1 class={styles.title}>项目</h1>
        <button
          type="button"
          class={styles.addBtn}
          onClick={() => setAddOpen(true)}
          data-testid="projects-add-btn"
          aria-label="注册项目"
        >
          + 注册
        </button>
      </header>
      <div class={styles.body}>
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
                  点 + 注册，或在 home 上跑{" "}
                  <span class="mono">fuxi project add ~/your-repo</span>
                </p>
              </div>
            }
          >
            <ul class={styles.list}>
              <For each={data()!.projects}>
                {(p) => <ProjectCard project={p} onChanged={() => void refetch()} />}
              </For>
            </ul>
          </Show>
        </Show>
      </div>
      <Show when={addOpen()}>
        <AddProjectModal
          onClose={() => setAddOpen(false)}
          onSuccess={() => {
            setAddOpen(false);
            void refetch();
          }}
        />
      </Show>
    </div>
  );
};

const ProjectCard: Component<{ project: ProjectView; onChanged: () => void }> = (
  props,
) => {
  const created = (): string => {
    const t = new Date(props.project.created_at);
    if (Number.isNaN(t.getTime())) return props.project.created_at;
    return t.toLocaleDateString("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
  };
  const { client } = useApi();
  const [confirming, setConfirming] = createSignal(false);
  const [deleting, setDeleting] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const onDelete = async (): Promise<void> => {
    setDeleting(true);
    setError(null);
    try {
      await client.removeProject(props.project.id);
      props.onChanged();
    } catch (e) {
      setError(e instanceof ApiError ? `${e.status}: ${e.message}` : String(e));
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
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
        <Show
          when={!confirming()}
          fallback={
            <span class={styles.confirmRow}>
              <span class={styles.confirmHint}>连同 sandboxes / 交付一并删？</span>
              <button
                type="button"
                class={styles.confirmCancel}
                onClick={() => setConfirming(false)}
                data-testid={`project-delete-cancel-${props.project.id}`}
              >
                取消
              </button>
              <button
                type="button"
                class={styles.confirmConfirm}
                disabled={deleting()}
                onClick={() => void onDelete()}
                data-testid={`project-delete-confirm-${props.project.id}`}
              >
                {deleting() ? "…" : "确认删除"}
              </button>
            </span>
          }
        >
          <button
            type="button"
            class={styles.deleteBtn}
            onClick={() => setConfirming(true)}
            data-testid={`project-delete-${props.project.id}`}
            aria-label={`删除 ${props.project.id}`}
          >
            删除
          </button>
        </Show>
      </div>
      <Show when={error()}>
        <p class={styles.errMsg} role="alert">
          {error()}
        </p>
      </Show>
    </li>
  );
};

const AddProjectModal: Component<{
  onClose: () => void;
  onSuccess: () => void;
}> = (props) => {
  const { client } = useApi();
  const [path, setPath] = createSignal("");
  const [name, setName] = createSignal("");
  const [branch, setBranch] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);

  const onSubmit = async (e: Event): Promise<void> => {
    e.preventDefault();
    if (!path().trim()) {
      setErr("项目路径不能为空");
      return;
    }
    setSubmitting(true);
    setErr(null);
    try {
      const req: AddProjectRequest = {
        canonical_path: path().trim(),
      };
      if (name().trim()) req.name = name().trim();
      if (branch().trim()) req.default_branch = branch().trim();
      await client.addProject(req);
      props.onSuccess();
    } catch (e) {
      if (e instanceof ApiError) {
        if (e.status === 409) setErr("已注册过（slug 撞名）");
        else if (e.status === 400) setErr(`参数无效：${e.message}`);
        else setErr(`失败 ${e.status}：${e.message}`);
      } else {
        setErr(String(e));
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div
      class={styles.modalScrim}
      data-testid="projects-add-modal"
      onClick={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
    >
      <form
        class={styles.modalCard}
        role="dialog"
        aria-label="注册项目"
        onSubmit={(e) => void onSubmit(e)}
      >
        <header class={styles.modalHead}>
          <h2 class={styles.modalTitle}>注册项目</h2>
          <button
            type="button"
            class={styles.modalClose}
            onClick={() => props.onClose()}
            aria-label="关闭"
            data-testid="projects-add-close"
          >
            ×
          </button>
        </header>
        <div class={styles.modalBody}>
          <label class={styles.field}>
            <span class={styles.fieldLabel}>项目路径（必填）</span>
            <input
              type="text"
              class={`${styles.fieldInput} mono`}
              placeholder="/Users/e0_7/erp"
              value={path()}
              onInput={(e) => setPath(e.currentTarget.value)}
              data-testid="projects-add-path"
              autofocus
            />
            <span class={styles.fieldHint}>
              fuxi-im 同机器的绝对路径，必须是已 init 的 git repo
            </span>
          </label>
          <label class={styles.field}>
            <span class={styles.fieldLabel}>名字 (slug)</span>
            <input
              type="text"
              class={`${styles.fieldInput} mono`}
              placeholder="留空从路径末段派生"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
              data-testid="projects-add-name"
            />
            <span class={styles.fieldHint}>仅 [a-z0-9_-]，1..=64 字符</span>
          </label>
          <label class={styles.field}>
            <span class={styles.fieldLabel}>默认分支</span>
            <input
              type="text"
              class={`${styles.fieldInput} mono`}
              placeholder="main"
              value={branch()}
              onInput={(e) => setBranch(e.currentTarget.value)}
              data-testid="projects-add-branch"
            />
          </label>
          <Show when={err()}>
            <p class={styles.errMsg} role="alert">
              {err()}
            </p>
          </Show>
        </div>
        <footer class={styles.modalFooter}>
          <button
            type="button"
            class={styles.btnSecondary}
            onClick={() => props.onClose()}
          >
            取消
          </button>
          <button
            type="submit"
            class={styles.btnPrimary}
            disabled={submitting()}
            data-testid="projects-add-submit"
          >
            {submitting() ? "注册中…" : "注册"}
          </button>
        </footer>
      </form>
    </div>
  );
};
