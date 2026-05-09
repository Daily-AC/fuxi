import { For, Show, createSignal, type Component } from "solid-js";
import { ApiError } from "~/lib/api";
import { useApi } from "./ApiProvider";
import type { Upload } from "~/types/api";
import styles from "./Composer.module.css";

// 底部 sticky composer · IM 标准位置。阶段 3 加附件管道。
// 状态机：
//   user 选 N 文件 → 立即 chip 列在 composer 上方（idle/uploading/done/error 单 chip 状态）
//   用户按"发送"或 Enter：
//     1) 串行上传所有 idle chip（已 done 的跳过；error 的不重试，由用户手动 ×）
//     2) 任一失败 → toast 红字，不发送 text；保留 chip 让用户决定
//     3) 全部 done 后调 onSubmit({ text, attachments }) 让父级发 intervene
//   text 空 + attachments 空 → 不能提交；text 空 + attachments 非空 → 可提交（纯附件消息）

export type ChipStatus = "idle" | "uploading" | "done" | "error";

export interface AttachmentChip {
  /** 本地 client id（区别于服务端 upload.id）。*/
  cid: string;
  file: File;
  status: ChipStatus;
  progress: number; // 0..1
  upload?: Upload; // done 后填
  error?: string;
}

export interface ComposerProps {
  /** 阶段 3：上传完成的 attachments 一并交给父级。父级负责 intervene 带 attachments。*/
  onSubmit: (text: string, attachments: Upload[]) => Promise<void>;
  placeholder?: string;
  disabled?: boolean;
}

export const Composer: Component<ComposerProps> = (props) => {
  const { client } = useApi();
  const [text, setText] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [chips, setChips] = createSignal<AttachmentChip[]>([]);
  const [errMsg, setErrMsg] = createSignal<string | null>(null);
  let fileInput: HTMLInputElement | undefined;

  const updateChip = (cid: string, patch: Partial<AttachmentChip>): void => {
    setChips((cs) => cs.map((c) => (c.cid === cid ? { ...c, ...patch } : c)));
  };

  /** v1-session19 #1 抽出复用：选择 / 粘贴 / 拖拽 共用同一推 chip 路径。 */
  const addFiles = (files: FileList | File[]): void => {
    if (!files || files.length === 0) return;
    const fresh: AttachmentChip[] = [];
    for (let i = 0; i < files.length; i += 1) {
      const f = files[i];
      if (!f) continue;
      fresh.push({
        cid: `c-${Date.now()}-${Math.random().toString(36).slice(2, 6)}-${i}`,
        file: f,
        status: "idle",
        progress: 0,
      });
    }
    if (fresh.length === 0) return;
    setChips([...chips(), ...fresh]);
  };

  const onFilesPicked = (e: Event): void => {
    const target = e.target as HTMLInputElement;
    const files = target.files;
    if (!files) return;
    addFiles(files);
    target.value = ""; // 允许相同文件再次选
  };

  /** Cmd/Ctrl+V 粘贴含图片 / 文件时拦截 default → 走 addFiles 上传管线。
   *  纯文本粘贴不动（textarea 默认行为接管）。详见 MentionComposer 同名 handler。 */
  const onPaste = (e: ClipboardEvent): void => {
    const cd = e.clipboardData;
    if (!cd) return;
    const files = cd.files;
    if (!files || files.length === 0) return;
    e.preventDefault();
    addFiles(files);
  };

  const removeChip = (cid: string): void => {
    setChips((cs) => cs.filter((c) => c.cid !== cid));
  };

  /** 串行上传所有 idle chip。已 done 的跳过；error 的不重试。
   *  返回 true 表示全部 done；false 表示有 error 或仍有 idle/uploading 异常。*/
  const uploadAll = async (): Promise<boolean> => {
    for (const c of chips()) {
      if (c.status === "done") continue;
      if (c.status === "error") return false; // 用户没清，不发送
      updateChip(c.cid, { status: "uploading", progress: 0, error: undefined });
      try {
        const up = await client.uploadFile(c.file, (ratio) => {
          updateChip(c.cid, { progress: ratio });
        });
        updateChip(c.cid, { status: "done", progress: 1, upload: up });
      } catch (err) {
        const msg =
          err instanceof ApiError
            ? `上传失败 (${err.status})`
            : err instanceof Error
              ? err.message
              : "上传失败";
        updateChip(c.cid, { status: "error", error: msg });
        return false;
      }
    }
    return true;
  };

  const canSend = (): boolean => {
    if (busy() || props.disabled) return false;
    const hasText = text().trim().length > 0;
    const hasChips = chips().length > 0;
    if (!hasText && !hasChips) return false;
    // 任一 chip 在 error 状态 → 必须先清掉
    if (chips().some((c) => c.status === "error")) return false;
    return true;
  };

  const send = async (e?: Event): Promise<void> => {
    e?.preventDefault();
    if (!canSend()) return;
    setBusy(true);
    setErrMsg(null);
    try {
      const allDone = chips().length === 0 ? true : await uploadAll();
      if (!allDone) {
        setErrMsg("有附件上传失败，请清理后再发送");
        return;
      }
      const ups: Upload[] = chips()
        .map((c) => c.upload)
        .filter((u): u is Upload => Boolean(u));
      const t = text().trim();
      await props.onSubmit(t, ups);
      setText("");
      setChips([]);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form class={styles.composer} onSubmit={send} role="form" aria-label="跟玄女说">
      <Show when={chips().length > 0 || errMsg()}>
        <div class={styles.chipsRow} data-testid="composer-chips">
          <For each={chips()}>
            {(c) => (
              <div
                class={styles.chip}
                classList={{
                  [styles.chipUploading ?? ""]: c.status === "uploading",
                  [styles.chipDone ?? ""]: c.status === "done",
                  [styles.chipError ?? ""]: c.status === "error",
                }}
                data-testid={`composer-chip-${c.cid}`}
                data-status={c.status}
              >
                <span class={styles.chipName} title={c.file.name}>
                  {c.file.name}
                </span>
                <span class={styles.chipMeta}>
                  <Show
                    when={c.status === "uploading"}
                    fallback={
                      <Show
                        when={c.status === "error"}
                        fallback={<span>{formatBytes(c.file.size)}</span>}
                      >
                        <span>{c.error}</span>
                      </Show>
                    }
                  >
                    <span>{Math.round(c.progress * 100)}%</span>
                  </Show>
                </span>
                <button
                  type="button"
                  class={styles.chipRemove}
                  onClick={() => removeChip(c.cid)}
                  aria-label="移除附件"
                  data-testid={`composer-chip-remove-${c.cid}`}
                  disabled={c.status === "uploading"}
                >
                  ×
                </button>
              </div>
            )}
          </For>
          <Show when={errMsg()}>
            <div class={styles.errBanner} role="alert">
              {errMsg()}
            </div>
          </Show>
        </div>
      </Show>
      <div class={styles.bar}>
        <button
          type="button"
          class={styles.attachBtn}
          onClick={() => fileInput?.click()}
          aria-label="添加附件"
          data-testid="composer-attach"
          disabled={busy() || props.disabled}
        >
          附件
        </button>
        <input
          ref={fileInput}
          type="file"
          multiple
          accept="image/*,application/pdf,text/*,application/json,application/zip,video/*"
          class={styles.fileInput}
          data-testid="composer-file-input"
          onChange={onFilesPicked}
        />
        <textarea
          class={styles.input}
          rows={1}
          placeholder={props.placeholder ?? "跟玄女说……"}
          value={text()}
          data-testid="composer-input"
          disabled={busy() || props.disabled}
          onInput={(e) => setText(e.currentTarget.value)}
          onPaste={onPaste}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button
          type="submit"
          class={styles.send}
          classList={{ [styles.sendActive ?? ""]: canSend() }}
          disabled={!canSend()}
          data-testid="composer-send"
          aria-label="发送"
        >
          发送
        </button>
      </div>
    </form>
  );
};

function formatBytes(b: number): string {
  if (!b) return "";
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / (1024 * 1024)).toFixed(1)} MB`;
}
