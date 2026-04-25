import { For, Show, createResource, createSignal, type Component, onMount } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { TaskCard } from "~/components/TaskCard";
import { cacheTasks, loadCachedTasks } from "~/lib/idb";
import type { TaskCard as TaskCardType } from "~/types/events";
import styles from "./TasksView.module.css";

// 主屏：root 任务卡片网格。运行中置顶 / 完成置灰沉底（决策 14 §A）。
export const TasksView: Component = () => {
  const { client } = useApi();
  const [cached, setCached] = createSignal<TaskCardType[]>([]);

  onMount(async () => {
    setCached(await loadCachedTasks());
  });

  const [tasks, { refetch }] = createResource(async () => {
    const r = await client.fetchTasks(true);
    await cacheTasks(r.tasks);
    return r.tasks;
  });

  const sorted = (): TaskCardType[] => {
    const list = tasks() ?? cached();
    const order: Record<string, number> = {
      running: 0,
      blocked: 1,
      pending: 2,
      failed: 3,
      done: 4,
    };
    return [...list].sort((a, b) => {
      const oa = order[a.status] ?? 99;
      const ob = order[b.status] ?? 99;
      if (oa !== ob) return oa - ob;
      return new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
    });
  };

  return (
    <section class={styles.view} data-testid="tasks-view">
      <Show when={tasks.error}>
        <div class={styles.errorBanner} role="alert">
          <span>加载失败：{String(tasks.error)}</span>
          <button class={styles.retry} onClick={() => refetch()} type="button">
            重试
          </button>
        </div>
      </Show>

      <Show
        when={sorted().length > 0}
        fallback={
          <div class={styles.empty}>
            <Show when={!tasks.loading} fallback={<span>正在拉取……</span>}>
              <p class={styles.emptyTitle}>当前没有任务</p>
              <p class={styles.emptyHint}>
                在上方"跟玄女说"输入派活内容，玄女会自动开 root task。
              </p>
            </Show>
          </div>
        }
      >
        <div class={styles.grid}>
          <For each={sorted()}>{(t) => <TaskCard task={t} />}</For>
        </div>
      </Show>
    </section>
  );
};
