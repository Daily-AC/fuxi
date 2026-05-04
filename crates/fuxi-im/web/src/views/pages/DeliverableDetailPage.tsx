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
import { Markdown } from "~/components/Markdown";
import type {
  DeliverableEntry,
  DeliverableFileMeta,
  DeliverableKind,
  DeliverableStatus,
} from "~/types/api";
import styles from "./DeliverableDetailPage.module.css";

// bug #76：预览扩展名分类
const TEXT_EXTS = ["md", "markdown", "txt", "log", "json", "csv", "tsv", "xml", "yaml", "yml", "rs", "ts", "tsx", "js", "jsx", "py", "sh", "html", "css"];
const MD_EXTS = ["md", "markdown"];
const IMG_EXTS = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico"];
const PREVIEW_BYTE_LIMIT = 512 * 1024; // 512 KB 文本上限——避免拉超大 log 卡浏览器

// 交付详情 Layer 2 · Decision 22 phase 3
//
// 数据源：GET /api/deliverables → 前端 filter (project, task) 拿同 task 多 entries。
// 文件：每条 entry 多个 file，统一展示成大列表 + sha256 全文 + 下载链接。
// 操作：accept (可选 accepted_to) / reject (可选 reason)。

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

export interface DeliverableDetailPageProps {
  project_id: string;
  task_id: string;
}

export const DeliverableDetailPage: Component<DeliverableDetailPageProps> = (
  props,
) => {
  const { client, navPop } = useApi();
  const [data, { refetch }] = createResource(() => client.fetchDeliverables());
  const myEntries = createMemo<DeliverableEntry[]>(() => {
    const all = data()?.deliverables ?? [];
    return all.filter(
      (e) => e.project === props.project_id && e.task === props.task_id,
    );
  });
  const status = createMemo<DeliverableStatus | null>(() => {
    const e = myEntries();
    if (e.length === 0) return null;
    const first = e[0];
    return first ? first.status : null;
  });

  // P1.4 体验：accept_to 默认值——按 project 分别 sticky（用户多项目时
  // 每个项目可能有不同的"收件目录"习惯）。`localStorage` 是浏览器局部，重装
  // PWA 失忆——但简单可靠不依赖后端，第一阶段够用。
  const acceptedToStorageKey = (): string =>
    `fuxi:acceptedTo:${props.project_id}`;
  const initialAcceptedTo = ((): string => {
    try {
      return localStorage.getItem(acceptedToStorageKey()) ?? "";
    } catch {
      return "";
    }
  })();
  const [acceptedTo, setAcceptedTo] = createSignal(initialAcceptedTo);
  const [reason, setReason] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);

  const onAccept = async (): Promise<void> => {
    setBusy(true);
    setErr(null);
    try {
      const target = acceptedTo().trim();
      await client.acceptDeliverable(
        props.project_id,
        props.task_id,
        target ? { accepted_to: target } : undefined,
      );
      // 只有真填了路径 + 后端没拒（成功落到 catch 之外）才记忆
      if (target) {
        try {
          localStorage.setItem(acceptedToStorageKey(), target);
        } catch {
          // localStorage 可能被禁——静默不挂主流程
        }
      }
      void refetch();
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
      const r = reason().trim();
      await client.rejectDeliverable(
        props.project_id,
        props.task_id,
        r ? { reason: r } : undefined,
      );
      void refetch();
    } catch (e) {
      setErr(e instanceof ApiError ? `${e.status}: ${e.message}` : String(e));
    } finally {
      setBusy(false);
    }
  };

  const taskShort = (): string =>
    props.task_id.length > 12 ? props.task_id.slice(0, 12) : props.task_id;

  return (
    <div class={styles.page} data-testid="page-deliverable-detail">
      <header class={styles.header}>
        <button
          type="button"
          class={styles.backBtn}
          onClick={() => navPop()}
          data-testid="deliverable-detail-back"
          aria-label="返回交付收件箱"
        >
          ‹ 交付
        </button>
        <h1 class={styles.title}>
          <span class={styles.titleProj}>{props.project_id}</span>
          <span class={styles.titleSep} aria-hidden="true">·</span>
          <span class={`${styles.titleTask} mono`}>task-{taskShort()}</span>
        </h1>
        <span class={styles.headerSpacer} aria-hidden="true" />
      </header>

      <div class={styles.body}>
        <Show
          when={data()}
          fallback={
            <p class={styles.muted} data-testid="deliverable-detail-loading">
              加载中…
            </p>
          }
        >
          <Show
            when={myEntries().length > 0}
            fallback={
              <div class={styles.empty} data-testid="deliverable-detail-empty">
                <p class={styles.emptyTitle}>无交付</p>
                <p class={styles.emptyHint}>
                  该 task 尚未产生 deliverable，或已被收件箱清理
                </p>
              </div>
            }
          >
            <Show when={status()}>
              <section
                class={`${styles.statusCard} ${styles[`statusCard_${status()}`] ?? ""}`}
                data-testid={`deliverable-detail-status-${status()}`}
              >
                <span class={styles.statusLabel}>当前状态</span>
                <span class={styles.statusValue}>
                  {STATUS_LABEL[status()!]}
                </span>
              </section>
            </Show>

            <For each={myEntries()}>
              {(entry) => <EntrySection entry={entry} />}
            </For>

            <Show when={status() === "pending"}>
              <section class={styles.actions}>
                <h2 class={styles.actionsTitle}>处理</h2>
                <div class={styles.actionField}>
                  <label class={styles.actionLabel} for="accepted-to">
                    接收到（可选 · 绝对路径）
                  </label>
                  <input
                    id="accepted-to"
                    type="text"
                    class={`${styles.actionInput} mono`}
                    placeholder="/Users/e0_7/写作"
                    value={acceptedTo()}
                    onInput={(e) => setAcceptedTo(e.currentTarget.value)}
                    data-testid="deliverable-accept-target"
                  />
                </div>
                <div class={styles.actionField}>
                  <label class={styles.actionLabel} for="reject-reason">
                    拒绝理由（可选）
                  </label>
                  <input
                    id="reject-reason"
                    type="text"
                    class={styles.actionInput}
                    placeholder="不符合期望 / 改动太大…"
                    value={reason()}
                    onInput={(e) => setReason(e.currentTarget.value)}
                    data-testid="deliverable-reject-reason"
                  />
                </div>
                <div class={styles.actionRow}>
                  <button
                    type="button"
                    class={styles.btnReject}
                    disabled={busy()}
                    onClick={() => void onReject()}
                    data-testid="deliverable-detail-reject"
                  >
                    拒绝
                  </button>
                  <button
                    type="button"
                    class={styles.btnAccept}
                    disabled={busy()}
                    onClick={() => void onAccept()}
                    data-testid="deliverable-detail-accept"
                  >
                    {busy() ? "…" : "接收"}
                  </button>
                </div>
                <Show when={err()}>
                  <p class={styles.errMsg} role="alert">
                    {err()}
                  </p>
                </Show>
              </section>
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
};

const EntrySection: Component<{ entry: DeliverableEntry }> = (props) => {
  const { client } = useApi();
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
  return (
    <section
      class={styles.entry}
      data-testid={`deliverable-detail-entry-${props.entry.kind}`}
    >
      <header class={styles.entryHead}>
        <span class={styles.entryKind}>
          {KIND_LABEL[props.entry.kind] ?? props.entry.kind}
        </span>
        <time class={styles.entryTime}>{produced()}</time>
      </header>
      <ul class={styles.fileList}>
        <For each={props.entry.files}>
          {(f) => (
            <FileRow
              file={f}
              project={props.entry.project}
              task={props.entry.task}
              downloadUrl={client.deliverableFileUrl(
                props.entry.project,
                props.entry.task,
                f.name,
              )}
              previewUrl={client.deliverableFilePreviewUrl(
                props.entry.project,
                props.entry.task,
                f.name,
              )}
            />
          )}
        </For>
      </ul>
    </section>
  );
};

const FileRow: Component<{
  file: DeliverableFileMeta;
  project: string;
  task: string;
  downloadUrl: string;
  previewUrl: string;
}> = (props) => {
  const ext = (): string =>
    props.file.name.toLowerCase().split(".").pop() ?? "";
  const isText = (): boolean => TEXT_EXTS.includes(ext());
  const isMd = (): boolean => MD_EXTS.includes(ext());
  const isImg = (): boolean => IMG_EXTS.includes(ext());
  const tooLargeForText = (): boolean =>
    isText() && props.file.size_bytes > PREVIEW_BYTE_LIMIT;
  const previewable = (): boolean => isImg() || (isText() && !tooLargeForText());

  return (
    <li
      class={styles.fileRow}
      data-testid={`deliverable-detail-file-${props.task}-${props.file.name}`}
    >
      <header class={styles.fileHeader}>
        <span class={styles.fileName}>{props.file.name}</span>
        <span class={`${styles.fileSize} mono`}>
          {formatBytes(props.file.size_bytes)}
        </span>
        <span class={`${styles.fileSha} mono`} title={`sha256: ${props.file.sha256}`}>
          {props.file.sha256.slice(0, 16)}…
        </span>
        <a
          class={styles.fileDownloadBtn}
          href={props.downloadUrl}
          download={props.file.name}
          data-testid={`deliverable-detail-download-${props.task}-${props.file.name}`}
          aria-label={`下载 ${props.file.name}`}
          title="下载"
        >
          ⬇
        </a>
      </header>
      <Show when={previewable()}>
        <FilePreview
          name={props.file.name}
          url={props.previewUrl}
          isMd={isMd()}
          isImg={isImg()}
          testid={`deliverable-detail-preview-${props.task}-${props.file.name}`}
        />
      </Show>
      <Show when={!previewable()}>
        <p
          class={styles.fileUnsupported}
          data-testid={`deliverable-detail-no-preview-${props.task}-${props.file.name}`}
        >
          {tooLargeForText()
            ? `文件 > ${Math.round(PREVIEW_BYTE_LIMIT / 1024)}KB，请下载查看`
            : "该格式暂不支持预览，请下载"}
        </p>
      </Show>
    </li>
  );
};

/** bug #76 内联预览：md → Markdown 渲染；其它文本 → <pre>；图片 → <img>。
 *  text 类走 fetch 拉文本，加载中 / 失败都不阻塞父组件其它部分的下载按钮。 */
const FilePreview: Component<{
  name: string;
  url: string;
  isMd: boolean;
  isImg: boolean;
  testid: string;
}> = (props) => {
  if (props.isImg) {
    return (
      <img
        class={styles.imgPreview}
        src={props.url}
        alt={props.name}
        data-testid={props.testid}
        loading="lazy"
      />
    );
  }
  // text/md：fetch 拉文本（credentials 必带——cookie 鉴权）。
  // jsdom test env 下 relative URL 会炸 → URL 标准化（test/production 双安全）。
  const [text] = createResource(async () => {
    const base =
      typeof location !== "undefined" && location.origin && location.origin !== "null"
        ? location.origin
        : "http://localhost";
    const abs = new URL(props.url, base).toString();
    const resp = await fetch(abs, { credentials: "include" });
    if (!resp.ok) throw new Error(`${resp.status} ${resp.statusText}`);
    return resp.text();
  });
  return (
    <Show
      when={text()}
      fallback={
        <p class={styles.previewLoading} data-testid={`${props.testid}-loading`}>
          {text.error ? `预览失败：${String(text.error)}` : "加载中…"}
        </p>
      }
    >
      <Show
        when={props.isMd}
        fallback={
          <pre class={styles.textPreview} data-testid={props.testid}>
            {text()}
          </pre>
        }
      >
        <Markdown class={styles.mdPreview} source={text() ?? ""} />
      </Show>
    </Show>
  );
};

function formatBytes(n: number): string {
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)}MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)}GB`;
}
