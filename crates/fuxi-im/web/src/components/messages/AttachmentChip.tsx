import { Show, type Component } from "solid-js";
import type { Upload } from "~/types/api";
import styles from "./AttachmentChip.module.css";

function formatBytes(b: number): string {
  if (!b) return "";
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / (1024 * 1024)).toFixed(1)} MB`;
}

function isImage(u: Upload): boolean {
  return /^image\//i.test(u.mime);
}

// 已上传完成的附件 chip · 渲染在消息 bubble 内（点击 → 直链 GET /api/uploads/:id）。
export const AttachmentChip: Component<{ upload: Upload }> = (props) => {
  const href = (): string => `/api/uploads/${encodeURIComponent(props.upload.id)}`;
  return (
    <a
      class={styles.chip}
      classList={{ [styles.image ?? ""]: isImage(props.upload) }}
      href={href()}
      target="_blank"
      rel="noopener noreferrer"
      data-testid={`attachment-chip-${props.upload.id}`}
    >
      <Show
        when={isImage(props.upload)}
        fallback={
          <div class={styles.fileBlock}>
            <span class={styles.fileIcon} aria-hidden="true">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none">
                <path
                  d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linejoin="round"
                />
                <path d="M14 2v6h6" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" />
              </svg>
            </span>
            <div class={styles.fileMeta}>
              <div class={styles.fileName}>{props.upload.name}</div>
              <div class={styles.fileBytes}>{formatBytes(props.upload.bytes)}</div>
            </div>
          </div>
        }
      >
        <img class={styles.thumb} src={href()} alt={props.upload.name} loading="lazy" />
        <div class={styles.imageCaption}>{props.upload.name}</div>
      </Show>
    </a>
  );
};
