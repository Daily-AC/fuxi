import { For, type Component } from "solid-js";
import type { DeliverableFileEntry, DeliverableMessage } from "~/messages";
import styles from "./DeliverableCard.module.css";

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const hh = d.getHours().toString().padStart(2, "0");
  const mm = d.getMinutes().toString().padStart(2, "0");
  return `${hh}:${mm}`;
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

// 文件类型 badge：按扩展给短文字角标。简约风格不用 emoji（feedback_no_emoji_tui
// 虽是 TUI 偏好但 PWA 同款简约更协调）。未知扩展给截短的 ext，无扩展给 "FILE"。
function fileBadge(name: string): string {
  const dot = name.lastIndexOf(".");
  if (dot < 0 || dot === name.length - 1) return "FILE";
  const ext = name.slice(dot + 1).toLowerCase();
  if (ext === "md" || ext === "markdown") return "MD";
  if (ext === "json") return "JSON";
  if (ext === "csv") return "CSV";
  if (ext === "tsv") return "TSV";
  if (ext === "pdf") return "PDF";
  if (["png", "jpg", "jpeg", "gif", "webp", "svg"].includes(ext)) return "IMG";
  if (["mp4", "mov", "avi", "webm"].includes(ext)) return "VID";
  if (["mp3", "wav", "flac", "ogg"].includes(ext)) return "AUD";
  if (["txt", "log"].includes(ext)) return "TXT";
  if (ext === "html" || ext === "htm") return "HTML";
  if (ext === "yaml" || ext === "yml") return "YAML";
  if (ext === "toml") return "TOML";
  return ext.toUpperCase().slice(0, 4);
}

// 5 类 deliverable_kind → 中文角标，对齐 sentinel addendum / Decision 13。
function kindLabel(kind: string): string {
  switch (kind) {
    case "research_summary":
      return "调研";
    case "code_change":
      return "代码";
    case "test_result":
      return "测试";
    case "decision_request":
      return "决策";
    case "error_block":
      return "报错";
    default:
      return kind || "交付";
  }
}

// 下载 URL = backend `GET /api/deliverables/:project/:task/files/:name`（已实装）。
// 段 B 加预览 modal 时切换到 preview/:name 路径，下载 button 仍走 files 路径。
function downloadUrl(msg: DeliverableMessage, file: DeliverableFileEntry): string {
  const project = encodeURIComponent(msg.project);
  const task = encodeURIComponent(msg.task);
  const name = encodeURIComponent(file.name);
  return `/api/deliverables/${project}/${task}/files/${name}`;
}

// P3 段 A · 门客 deliverable 镜像到对话流的附件卡。结构：
//   ─────────────────────────────────────
//   鲁班 · [调研]                      14:32
//   ─────────────────────────────────────
//   [MD]  report.md         1.2 KB  [下载]
//   [CSV] data.csv          45 KB   [下载]
//   ─────────────────────────────────────
// 多文件场景每行独立可下载。点击 file row 触发预览 modal 是段 B 的事，v1 段 A
// 仅显「下载」按钮——`<a download>` 走浏览器默认下载行为。
export const DeliverableCard: Component<{ msg: DeliverableMessage }> = (props) => {
  return (
    <div class={styles.row} data-testid="msg-deliverable" data-msg-id={props.msg.id}>
      <div class={styles.head}>
        <span class={styles.name}>{props.msg.role_display ?? "门客"}</span>
        <span class={styles.sep}>·</span>
        <span class={styles.kindBadge}>{kindLabel(props.msg.deliverable_kind)}</span>
        <time class={styles.time}>{fmtTime(props.msg.ts)}</time>
      </div>
      <div class={styles.card}>
        <For each={props.msg.files}>
          {(file) => (
            <div class={styles.fileRow} data-testid="msg-deliverable-row">
              <span class={styles.badge}>{fileBadge(file.name)}</span>
              <span class={styles.filename} title={file.name}>
                {file.name}
              </span>
              <span class={styles.size}>{fmtSize(file.size_bytes)}</span>
              <a
                class={styles.download}
                href={downloadUrl(props.msg, file)}
                download={file.name}
                data-testid="msg-deliverable-download"
              >
                下载
              </a>
            </div>
          )}
        </For>
      </div>
    </div>
  );
};
