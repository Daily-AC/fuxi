import {
  For,
  Show,
  createMemo,
  createSignal,
  type Component,
} from "solid-js";
import { ApiError } from "~/lib/api";
import { useApi } from "./ApiProvider";
import { MentionAutocomplete } from "./MentionAutocomplete";
import { MentionChip } from "./MentionChip";
import {
  MULTI_MENTION_WARNING,
  MULTI_NODE_WARNING,
  MULTI_PROJECT_WARNING,
  fuzzyMatch,
  serializeComposer,
  type ComposerSegment,
  type MentionCandidate,
  type MentionChipToken,
  type SerializedIntervene,
} from "~/lib/mentions";
import { pushToast } from "~/lib/toast";
import type { Upload } from "~/types/api";
import styles from "./MentionComposer.module.css";

// MentionComposer · v3 #N5' / #40 (玄女 tab) + #N4' / #39 (任务 thread) 共用
// 设计 spec: 2026-04-26-im-tab-bar-task-thread-design.md §C / §"composer @ 机制"
//
// 数据模型：
//   - 内部状态 segments: ComposerSegment[]（text 段 + chip 段交错）
//   - chip 是不可分割 token；UI 上 inline-block，contentEditable=false（光标越过整体）
//   - 编辑现状文本只走最后一段 text；按 @ 进 query 模式 → 选候选 → 转换该段尾部 query 为 chip
//
// v1 简化（spec §多 @ 处理）：
//   - 多 chip 允许；send 时 mentions[0]=target，多 chip 时 toast 警示
//   - 中文输入法不触发 @（compositionstart/end 期间 @ 不进 query）
//   - autocomplete 候选 = parent 传入；过滤用 fuzzyMatch + 输入的 query
//
// 实装心智：editor 受控渲染 segments；用户键盘修改最末 text 段（输入 / 退格）。
// 为了 jsdom-friendly + 避免 contentEditable 跨浏览器坑，input 主用 textarea-like 流但
// chip 显示在 editor row 中作为 inline-block 元素（光标只在末段 text 内移动）。

export interface MentionComposerProps {
  /** 父级负责拉候选（玄女 tab 走 alive workers，任务 thread 走任务成员）。*/
  candidates: MentionCandidate[];
  /** 可选 · 没 @ 时的 fallback target；省略 = backend 走玄女默认（v3 玄女 tab 主路）。*/
  defaultTargetAgentId?: string;
  /** placeholder 文案（动态：默认对玄女 / 对鲁班）。*/
  placeholder?: string;
  disabled?: boolean;
  /** send 回调：父级负责拼 intervene 请求。*/
  onSubmit(req: SerializedIntervene): Promise<void>;
}

/** 附件 chip 状态机（v3 #46 加 / #50 改"选完立即上传"）。
 *  - uploading: 选完立即并发触发上传，chip 立刻显进度（不再等 send）
 *  - done: 上传完成，chip 显文件大小，可被 send
 *  - error: 上传失败，chip 显错误 + ✕ 让用户清；canSend=false 直到清掉
 *  v3 #50 删了 idle 状态：chip 一出现就直接 uploading。 */
type AttachStatus = "uploading" | "done" | "error";

interface AttachChip {
  /** 本地 client id（区别于服务端 upload.id）。*/
  cid: string;
  file: File;
  status: AttachStatus;
  progress: number; // 0..1
  upload?: Upload; // done 后填
  error?: string;
  /** v3 #50：选完立即上传，chip 持有自己的 AbortController；removeAttach 时 abort。*/
  controller: AbortController;
}

export const MentionComposer: Component<MentionComposerProps> = (props) => {
  const { client } = useApi();
  const [segments, setSegments] = createSignal<ComposerSegment[]>([
    { kind: "text", text: "" },
  ]);
  const [busy, setBusy] = createSignal(false);
  const [query, setQuery] = createSignal<string | null>(null); // null = 不在 @ 模式
  const [hi, setHi] = createSignal(0);
  const [attachChips, setAttachChips] = createSignal<AttachChip[]>([]);
  let composing = false; // IME 中文输入态
  let fileInput: HTMLInputElement | undefined;

  /** 把最末 text 段的内容写入 segments。
   *  保证：segments 末尾必为 text 段（即使空）。*/
  const setTailText = (text: string): void => {
    setSegments((prev) => {
      const out = prev.slice();
      const last = out[out.length - 1];
      if (last && last.kind === "text") {
        out[out.length - 1] = { kind: "text", text };
      } else {
        out.push({ kind: "text", text });
      }
      return out;
    });
  };

  const tailText = (): string => {
    const last = segments()[segments().length - 1];
    return last && last.kind === "text" ? last.text : "";
  };

  /** 候选过滤 = fuzzyMatch(props.candidates, query) */
  const filtered = createMemo<MentionCandidate[]>(() => {
    const q = query();
    if (q === null) return [];
    return fuzzyMatch(props.candidates, q);
  });

  const closeAutocomplete = (): void => {
    setQuery(null);
    setHi(0);
  };

  /** 选中候选 → 替换最末 text 段内 "@<query>" 为 chip token + 起新空 text 段。
   *  #60：candidate.kind 透传到 chip token（worker → mentions 路径；node → pinned_node 路径）。*/
  const onPickCandidate = (c: MentionCandidate): void => {
    setSegments((prev) => {
      const out = prev.slice();
      const last = out[out.length - 1];
      if (last && last.kind === "text") {
        // 砍掉 @ + query 部分（@ 的位置 = 末尾 - (query.length+1)）
        const q = query() ?? "";
        const cut = last.text.length - (q.length + 1);
        const before = cut >= 0 ? last.text.slice(0, cut) : last.text;
        out[out.length - 1] = { kind: "text", text: before };
      }
      const chip: MentionChipToken = {
        agent_id: c.agent_id,
        role: c.role,
        role_display: c.role_display,
        kind: c.kind ?? "worker",
      };
      out.push({ kind: "chip", chip });
      out.push({ kind: "text", text: " " }); // chip 后留个空格方便继续输入
      return out;
    });
    closeAutocomplete();
  };

  /** #60：唯一 key 是 (kind, id) — agent_id / node_id / project_slug 理论上可冲，按 kind 区分匹配。*/
  const onRemoveChip = (
    agent_id: string,
    kind: "worker" | "node" | "project" = "worker",
  ): void => {
    setSegments((prev) => {
      const out: ComposerSegment[] = [];
      let removed = false;
      for (const seg of prev) {
        const segKind = seg.kind === "chip" ? (seg.chip.kind ?? "worker") : null;
        if (
          !removed
          && seg.kind === "chip"
          && seg.chip.agent_id === agent_id
          && segKind === kind
        ) {
          removed = true;
          continue;
        }
        const last = out[out.length - 1];
        if (last && last.kind === "text" && seg.kind === "text") {
          out[out.length - 1] = { kind: "text", text: last.text + seg.text };
        } else {
          out.push(seg);
        }
      }
      const last = out[out.length - 1];
      if (!last || last.kind !== "text") out.push({ kind: "text", text: "" });
      return out;
    });
  };

  // ===== 附件管道（v3 #46 + #50 改"选完立即并发上传"）=====

  const updateAttach = (cid: string, patch: Partial<AttachChip>): void => {
    setAttachChips((cs) => cs.map((c) => (c.cid === cid ? { ...c, ...patch } : c)));
  };

  /** v3 #50：选完立即异步上传单个 chip · 不阻塞 onFilesPicked 返回 · progress 流式 update。 */
  const startUploadFor = async (chip: AttachChip): Promise<void> => {
    try {
      const up = await client.uploadFile(
        chip.file,
        (ratio) => updateAttach(chip.cid, { progress: ratio }),
        chip.controller.signal,
      );
      updateAttach(chip.cid, { status: "done", progress: 1, upload: up });
    } catch (err) {
      // abort 走过这里 (ApiError status=0 message="aborted")。chip 已被 removeAttach 提前 splice 掉，
      // updateAttach 的 cs.map 找不到 cid → noop，所以 abort 既不留 error chip 也不残留 done。
      const msg =
        err instanceof ApiError
          ? err.status === 0
            ? err.message
            : `上传失败 (${err.status})`
          : err instanceof Error
            ? err.message
            : "上传失败";
      // 已被 abort 取消的 chip 不留 error 状态（chip 早被 splice）
      const stillExists = attachChips().some((c) => c.cid === chip.cid);
      if (stillExists && msg !== "aborted") {
        updateAttach(chip.cid, { status: "error", error: msg });
      }
    }
  };

  const onFilesPicked = (e: Event): void => {
    const target = e.target as HTMLInputElement;
    const files = target.files;
    if (!files) return;
    const fresh: AttachChip[] = [];
    for (let i = 0; i < files.length; i += 1) {
      const f = files[i];
      if (!f) continue;
      fresh.push({
        cid: `c-${Date.now()}-${Math.random().toString(36).slice(2, 6)}-${i}`,
        file: f,
        status: "uploading",
        progress: 0,
        controller: new AbortController(),
      });
    }
    setAttachChips([...attachChips(), ...fresh]);
    target.value = ""; // 允许相同文件再次选
    // 并发上传所有新 chip · 不 await（不阻塞 UI）
    for (const c of fresh) void startUploadFor(c);
  };

  const removeAttach = (cid: string): void => {
    // v3 #50：上传中删 chip = abort xhr · 让 backend 不收完整文件
    const c = attachChips().find((x) => x.cid === cid);
    if (c && c.status === "uploading") c.controller.abort();
    setAttachChips((cs) => cs.filter((x) => x.cid !== cid));
  };

  /** 处理输入键 · 输入框 onInput · 我们用受控的"末段 text"模型，输入直接走 setTailText。*/
  const onInput = (e: InputEvent & { currentTarget: HTMLTextAreaElement }): void => {
    if (composing) return; // IME 中不更新模型
    const v = e.currentTarget.value;
    setTailText(v);
    // 检测 @ 模式：找最后一个 @ 在末段 text 中的位置；若 @ 后到末尾无空格 → query=该后缀
    const at = v.lastIndexOf("@");
    if (at < 0) {
      closeAutocomplete();
      return;
    }
    const after = v.slice(at + 1);
    if (/\s/.test(after)) {
      closeAutocomplete();
      return;
    }
    setQuery(after);
    setHi(0);
  };

  const onCompositionStart = (): void => {
    composing = true;
  };

  const onCompositionEnd = (
    e: CompositionEvent & { currentTarget: HTMLTextAreaElement },
  ): void => {
    composing = false;
    // 提交后再走一次 onInput 同步 model
    onInput(e as unknown as InputEvent & { currentTarget: HTMLTextAreaElement });
  };

  const onKeyDown = (e: KeyboardEvent & { currentTarget: HTMLTextAreaElement }): void => {
    // Backspace 在末段 text 为空时删除前面的 chip（chip 不可分割 token 体验）
    if (e.key === "Backspace" && tailText() === "") {
      const segs = segments();
      const beforeTail = segs[segs.length - 2];
      if (beforeTail && beforeTail.kind === "chip") {
        e.preventDefault();
        onRemoveChip(beforeTail.chip.agent_id, beforeTail.chip.kind ?? "worker");
        return;
      }
    }
    // Enter（无 shift）发送；autocomplete visible 时让 autocomplete 接管 Enter（在 MentionAutocomplete 内监听 document）
    if (e.key === "Enter" && !e.shiftKey && !composing && query() === null) {
      e.preventDefault();
      void send();
    }
  };

  /** UI 层"send 看上去能不能按"——uploading/error 时按钮 *仍然能按*（不 disabled），
   *  让 send() 走 toast 路径告诉用户为啥发不出去（spec §"否则禁 send + toast"）。 */
  const canSendUI = (): boolean => {
    if (busy() || props.disabled) return false;
    const hasText = segments().some((s) => s.kind === "text" && s.text.trim() !== "");
    const hasChip = segments().some((s) => s.kind === "chip");
    const hasAttach = attachChips().length > 0;
    return hasText || hasChip || hasAttach;
  };

  /** send() 内真正决定能不能发的版本（chip status 也得全 done）。 */
  const canSend = (): boolean => {
    if (!canSendUI()) return false;
    if (attachChips().some((c) => c.status !== "done")) return false;
    return true;
  };

  const reset = (): void => {
    setSegments([{ kind: "text", text: "" }]);
    setAttachChips([]);
    closeAutocomplete();
  };

  const send = async (): Promise<void> => {
    // v3 #50：附件状态拦截放在 canSend 之外做差异化 toast 文案
    if (busy() || props.disabled) return;
    const hasError = attachChips().some((c) => c.status === "error");
    const hasUploading = attachChips().some((c) => c.status === "uploading");
    if (hasError) {
      pushToast("有附件上传失败，请清掉再发", "error");
      return;
    }
    if (hasUploading) {
      pushToast("等附件传完再发", "warn");
      return;
    }
    if (!canSend()) return;
    setBusy(true);
    try {
      const req = serializeComposer(segments(), props.defaultTargetAgentId);
      const ups: Upload[] = attachChips()
        .map((c) => c.upload)
        .filter((u): u is Upload => Boolean(u));
      if (ups.length > 0) req.attachments = ups.map((u) => u.id);
      if (req.multi) {
        pushToast(MULTI_MENTION_WARNING, "warn");
      }
      if (req.multi_node) {
        pushToast(MULTI_NODE_WARNING, "warn");
      }
      if (req.multi_project) {
        pushToast(MULTI_PROJECT_WARNING, "warn");
      }
      await props.onSubmit(req);
      reset();
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      class={styles.composer}
      data-testid="mention-composer"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <MentionAutocomplete
        visible={query() !== null}
        candidates={filtered()}
        highlightedIndex={hi()}
        onSelect={onPickCandidate}
        onCancel={closeAutocomplete}
        onMoveSelection={(d) => {
          const list = filtered();
          if (list.length === 0) return;
          setHi((cur) => {
            const next = cur + d;
            if (next < 0) return list.length - 1;
            if (next >= list.length) return 0;
            return next;
          });
        }}
      />
      {/* 已存在的 chip 显在编辑器上方一行（v1 简化：不真做 inline contentEditable）。
          spec §C 钉了"光标只能在 chip 前后" —— 我们把 chip 维持成顺序集合放上一行，
          删除走 ✕（chip 不可分割 token 体验仍达成：用户无法把光标插到 chip 中间）。*/}
      <Show when={segments().some((s) => s.kind === "chip")}>
        <div class={styles.bar} data-testid="composer-chips" style={{ "min-height": "auto" }}>
          {segments()
            .filter((s): s is { kind: "chip"; chip: MentionChipToken } => s.kind === "chip")
            .map((s) => (
              <MentionChip
                agent_id={s.chip.agent_id}
                role={s.chip.role}
                role_display={s.chip.role_display}
                kind={s.chip.kind ?? "worker"}
                removable
                onRemove={() => onRemoveChip(s.chip.agent_id, s.chip.kind ?? "worker")}
              />
            ))}
        </div>
      </Show>
      {/* 附件 chip 行 · v3 #46 · 上传中显进度 / done 显大小 / error 显消息 + ✕ 删 */}
      <Show when={attachChips().length > 0}>
        <div class={styles.attachRow} data-testid="composer-attach-chips">
          <For each={attachChips()}>
            {(c) => (
              <div
                class={styles.attachChip}
                classList={{
                  [styles.attachChipUploading ?? ""]: c.status === "uploading",
                  [styles.attachChipDone ?? ""]: c.status === "done",
                  [styles.attachChipError ?? ""]: c.status === "error",
                }}
                data-testid={`composer-attach-${c.cid}`}
                data-status={c.status}
                title={c.error ?? c.file.name}
              >
                <span class={styles.attachName}>{c.file.name}</span>
                <Show
                  when={c.status === "uploading"}
                  fallback={
                    <Show
                      when={c.status === "error"}
                      fallback={<span class={styles.attachMeta}>{formatBytes(c.file.size)}</span>}
                    >
                      <span class={styles.attachMeta}>{c.error ?? "失败"}</span>
                    </Show>
                  }
                >
                  <span class={styles.attachMeta}>{Math.round(c.progress * 100)}%</span>
                </Show>
                <button
                  type="button"
                  class={styles.attachRemove}
                  aria-label={`移除附件 ${c.file.name}`}
                  data-testid={`composer-attach-remove-${c.cid}`}
                  onClick={() => removeAttach(c.cid)}
                >
                  ×
                </button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <div class={styles.bar}>
        <button
          type="button"
          class={styles.attachBtn}
          onClick={() => fileInput?.click()}
          aria-label="添加附件"
          data-testid="composer-attach-btn"
          disabled={busy() || props.disabled}
        >
          +
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
          class={styles.editor}
          value={tailText()}
          placeholder={props.placeholder ?? "对玄女说... (@ 角色或节点 · Enter 发送 · Shift+Enter 换行)"}
          data-testid="mention-editor"
          disabled={busy() || props.disabled}
          rows={1}
          // P1.5 多行：Enter=发送，Shift+Enter=换行（textarea 默认行为）。
          // auto-grow：每次 input 重设 height 让 textarea 跟内容长大；max-height
          // 由 CSS 控制（5 行后开始内部 scroll）。
          ref={(el) => {
            const grow = (): void => {
              el.style.height = "auto";
              el.style.height = `${el.scrollHeight}px`;
            };
            el.addEventListener("input", grow);
            queueMicrotask(grow);
          }}
          onInput={onInput}
          onCompositionStart={onCompositionStart}
          onCompositionEnd={onCompositionEnd}
          onKeyDown={onKeyDown}
          autocomplete="off"
        />
        <button
          type="submit"
          class={styles.send}
          classList={{ [styles.sendActive ?? ""]: canSend() }}
          disabled={!canSendUI()}
          data-testid="mention-send"
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
