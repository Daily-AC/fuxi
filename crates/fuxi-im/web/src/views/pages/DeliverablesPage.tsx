import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { ApiError } from "~/lib/api";
import { useApi } from "~/components/ApiProvider";
import type {
  DeliverableEntry,
  DeliverableFileMeta,
  DeliverableKind,
  DeliverableStatus,
} from "~/types/api";
import styles from "./DeliverablesPage.module.css";

// 交付 tab · Decision 22 phase 1
//
// PWA 视角的"门客交活给我"收件箱。
// 数据源：GET /api/deliverables → DeliverablesResponse{ deliverables: [...] }
// 文件下载：client.deliverableFileUrl(project, task, name) → 直链 <a>。
//
// v1：纯展示 + 下载。Accept / Reject 两个动作 phase 2 加（需要 POST 端点）。
// 按 produced_at 倒序（后端已排），按 (project, task) 聚合显示——同一 task 的
// 多次 produce 集中展示。

const KIND_LABEL: Record<DeliverableKind, string> = {
  research_summary: "调研",
  code_change: "代码改动",
  test_result: "测试结果",
  decision_request: "决策请示",
  error_block: "错误阻塞",
};

const STATUS_LABEL: Record<DeliverableStatus, string> = {
  pending: "待处理",
  accepted: "已接收",
  rejected: "已拒绝",
  expired: "已过期",
};

export const DeliverablesPage: Component = () => {
  const { client } = useApi();
  const [data, { refetch }] = createResource(() => client.fetchDeliverables());

  return (
    <div class={styles.page} data-testid="page-deliverables">
      <header class={styles.header}>
        <h1 class={styles.title}>交付</h1>
      </header>
      <div class={styles.body}>
        <Show
          when={data()}
          fallback={
            <Show
              when={data.error}
              fallback={
                <p class={styles.muted} data-testid="deliverables-loading">
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
            when={data()!.deliverables.length > 0}
            fallback={
              <div class={styles.empty} data-testid="deliverables-empty">
                <p class={styles.emptyTitle}>暂无交付</p>
                <p class={styles.emptyHint}>门客把活做完会在这里出现</p>
              </div>
            }
          >
            <ul class={styles.list}>
              <For each={data()!.deliverables}>
                {(e) => <EntryCard entry={e} onChanged={() => void refetch()} />}
              </For>
            </ul>
          </Show>
        </Show>
      </div>
    </div>
  );
};

const EntryCard: Component<{ entry: DeliverableEntry; onChanged: () => void }> = (
  props,
) => {
  const produced = (): string => {
    const t = new Date(props.entry.produced_at);
    if (Number.isNaN(t.getTime())) return props.entry.produced_at;
    return t.toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  };
  const taskShort = createMemo<string>(() => {
    const t = props.entry.task;
    return t.length > 8 ? t.slice(0, 8) : t;
  });
  const { client } = useApi();
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);

  const onAccept = async (): Promise<void> => {
    setBusy(true);
    setErr(null);
    try {
      await client.acceptDeliverable(props.entry.project, props.entry.task);
      props.onChanged();
    } catch (e) {
      setErr(e instanceof ApiError ? `${e.status}: ${e.message}` : String(e));
    } finally {
      setBusy(false);
    }
  };
  const onReject = async (): Promise<void> => {
    setBusy(true);
    setErr(null);
    try {
      await client.rejectDeliverable(props.entry.project, props.entry.task);
      props.onChanged();
    } catch (e) {
      setErr(e instanceof ApiError ? `${e.status}: ${e.message}` : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <li
      class={styles.card}
      data-testid={`deliverable-card-${props.entry.task}-${props.entry.kind}`}
      data-status={props.entry.status}
    >
      <div class={styles.cardHead}>
        <span class={styles.cardKind}>
          {KIND_LABEL[props.entry.kind] ?? props.entry.kind}
        </span>
        <span class={styles.cardMeta}>
          <span class={styles.metaProj}>{props.entry.project}</span>
          <span class={styles.metaSep} aria-hidden="true">·</span>
          <span class={`${styles.metaTask} mono`}>task-{taskShort()}</span>
          <span class={styles.metaSep} aria-hidden="true">·</span>
          <time class={styles.metaTime}>{produced()}</time>
        </span>
      </div>
      <ul class={styles.files}>
        <For each={props.entry.files}>
          {(f) => (
            <FileRow
              file={f}
              project={props.entry.project}
              task={props.entry.task}
            />
          )}
        </For>
      </ul>
      <div class={styles.cardActions}>
        <Show
          when={props.entry.status === "pending"}
          fallback={
            <span
              class={`${styles.statusBadge} ${
                styles[`status_${props.entry.status}`] ?? ""
              }`}
              data-testid={`deliverable-status-${props.entry.task}`}
            >
              {STATUS_LABEL[props.entry.status]}
            </span>
          }
        >
          <button
            type="button"
            class={styles.btnReject}
            disabled={busy()}
            onClick={() => void onReject()}
            data-testid={`deliverable-reject-${props.entry.task}`}
          >
            拒绝
          </button>
          <button
            type="button"
            class={styles.btnAccept}
            disabled={busy()}
            onClick={() => void onAccept()}
            data-testid={`deliverable-accept-${props.entry.task}`}
          >
            {busy() ? "…" : "接收"}
          </button>
        </Show>
      </div>
      <Show when={err()}>
        <p class={styles.errMsg} role="alert">
          {err()}
        </p>
      </Show>
    </li>
  );
};

const FileRow: Component<{
  file: DeliverableFileMeta;
  project: string;
  task: string;
}> = (props) => {
  const { client } = useApi();
  const url = (): string =>
    client.deliverableFileUrl(props.project, props.task, props.file.name);
  const sizeStr = (): string => formatBytes(props.file.size_bytes);
  const shaShort = (): string => props.file.sha256.slice(0, 12);
  return (
    <li class={styles.fileRow}>
      <a
        class={styles.fileLink}
        href={url()}
        download={props.file.name}
        data-testid={`deliverable-file-${props.task}-${props.file.name}`}
      >
        <span class={styles.fileName}>{props.file.name}</span>
        <span class={styles.fileMeta}>
          <span class={`${styles.fileSize} mono`}>{sizeStr()}</span>
          <span class={styles.metaSep} aria-hidden="true">·</span>
          <span class={`${styles.fileSha} mono`} title={`sha256: ${props.file.sha256}`}>
            {shaShort()}
          </span>
        </span>
      </a>
    </li>
  );
};

function formatBytes(n: number): string {
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)}MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)}GB`;
}
