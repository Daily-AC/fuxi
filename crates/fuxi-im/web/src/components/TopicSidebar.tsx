import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { ApiError } from "~/lib/api";
import { pushToast } from "~/lib/toast";
import type { TopicView } from "~/types/api";
import styles from "./TopicSidebar.module.css";

// fuxi-core::TopicId::general() 的常量 UUID——和后端 schema 对齐。
// 用户体验：归档 general 没意义（系统兜底），UI 直接 disable 归档按钮。
const GENERAL_TOPIC_ID = "00000000-0000-0000-0000-000000000001";

// Phase 1 · 桌面 left sidebar / 移动端左滑抽屉
//
// 设计源：docs/handoff/v1-session19.md §2-§4
//   - 决策 3：不暴露 pin 到顶（v2 再做）
//   - 决策 4：移动端左滑抽屉而非底 tab，主屏 100% 给对话
//   - 决策 6：归档不删
//   - 决策 7：跨 topic broadcast 阈值 backend 兜
//
// 行为：
//   - 30s 轮询 fetchTopics 刷新列表（topic 切换是低频，不开 WebSocket）
//   - 切换：调用 switchTopic（5-15s），按钮 disabled + spinner，期间禁止再点其他 topic
//   - 新建：prompt 弹输入 title，trim 后非空 → POST，成功后自动 switch 进去
//   - 归档：行右侧 ⋯ 菜单（hover 出），点 → confirm → POST archive → refetch
//   - 当前 topic_id 全局态由 ApiProvider 管，切换成功后 setCurrentTopicId 推全局
//     → XuannvPage 感知 topic 变化重新拉历史 + 重连 WS

const POLL_INTERVAL_MS = 30_000;

/** sidebar 行排序：按 last_active_at desc。归档项不在默认 list（include_archived=0）。 */
function sortTopics(list: TopicView[]): TopicView[] {
  return list.slice().sort((a, b) => {
    const ta = Date.parse(a.last_active_at);
    const tb = Date.parse(b.last_active_at);
    return tb - ta;
  });
}

export const TopicSidebar: Component = () => {
  const {
    client,
    currentTopicId,
    setCurrentTopicId,
    sidebarOpen,
    setSidebarOpen,
    setIsSwitchingTopic,
  } = useApi();

  const [data, { refetch }] = createResource(() => client.fetchTopics(false));
  const [switching, setSwitching] = createSignal<string | null>(null);
  const [creating, setCreating] = createSignal(false);
  const [menuOpenFor, setMenuOpenFor] = createSignal<string | null>(null);
  // bug D · IM 风格新建话题 modal 取代 window.prompt
  const [createOpen, setCreateOpen] = createSignal(false);
  // 归档确认弹窗的目标——非空 = 弹窗打开。同源理由：替换 window.confirm。
  const [archiveTarget, setArchiveTarget] = createSignal<{
    id: string;
    title: string;
  } | null>(null);
  const [archiving, setArchiving] = createSignal(false);

  // 初始填全局 currentTopicId——首次 fetchTopics 返 current_topic_id 时回写 context。
  // 若用户在另一 client 切了 topic，30s 后下一次 poll 也会同步过来。
  createEffect(() => {
    const d = data();
    if (d && d.current_topic_id && d.current_topic_id !== currentTopicId()) {
      setCurrentTopicId(d.current_topic_id);
    }
  });

  // 30s 轮询；切换后立即 refetch（switchTopic 完成回调里手动调）。
  onMount(() => {
    const t = setInterval(() => {
      void refetch();
    }, POLL_INTERVAL_MS);
    onCleanup(() => clearInterval(t));
  });

  const topics = createMemo<TopicView[]>(() => sortTopics(data()?.topics ?? []));

  const handleSwitch = async (id: string): Promise<void> => {
    if (switching()) return; // 期间禁止再点
    if (id === currentTopicId()) {
      // 已是当前 topic——移动端关掉抽屉即可
      setSidebarOpen(false);
      return;
    }
    setSwitching(id);
    setIsSwitchingTopic(true);
    try {
      const resp = await client.switchTopic(id);
      setCurrentTopicId(resp.current_topic_id);
      await refetch();
      setSidebarOpen(false);
    } catch (err) {
      const msg =
        err instanceof ApiError
          ? err.status === 503
            ? "玄女切换服务未就绪（xuannv_switcher 未注入）"
            : `切换失败 (${err.status})`
          : err instanceof Error
            ? err.message
            : "切换失败";
      pushToast(msg, "error");
    } finally {
      setSwitching(null);
      setIsSwitchingTopic(false);
    }
  };

  /** 接 modal 提交：trim/长度校验 → POST createTopic → refetch → 自动 switch 进新话题。
   *  bug B：整个流程期间 setIsSwitchingTopic(true)，XuannvPage 全程显 overlay。 */
  const submitCreate = async (rawTitle: string): Promise<void> => {
    const title = rawTitle.trim();
    if (title === "") {
      pushToast("标题不能为空", "warn");
      return;
    }
    if ([...title].length > 80) {
      pushToast("标题上限 80 字", "warn");
      return;
    }
    if (creating()) return;
    setCreating(true);
    setIsSwitchingTopic(true);
    try {
      const t = await client.createTopic({ title });
      setCreateOpen(false);
      await refetch();
      // 自动 switch 到新建的 topic——用户建 = 想进。handleSwitch 内部也会 set/clear
      // isSwitchingTopic，外层这一对覆盖整个 create→switch 全程不闪烁。
      await handleSwitch(t.id);
    } catch (err) {
      const msg =
        err instanceof ApiError ? `建话题失败 (${err.status})` :
        err instanceof Error ? err.message : "建话题失败";
      pushToast(msg, "error");
    } finally {
      setCreating(false);
      setIsSwitchingTopic(false);
    }
  };

  /** ⋯ 菜单点"归档" → 仅开 modal；真执行在 confirmArchive 里。
   *  general 在前端层硬拦，不进 modal 直接 toast 拒绝。 */
  const handleArchive = (id: string, title: string): void => {
    setMenuOpenFor(null);
    if (id === GENERAL_TOPIC_ID) {
      pushToast("默认 general 话题为系统兜底，不可归档", "warn");
      return;
    }
    setArchiveTarget({ id, title });
  };

  /** Modal 确认按钮回调：执行归档（含"当前话题先切回 general"路径）。 */
  const confirmArchive = async (): Promise<void> => {
    const target = archiveTarget();
    if (!target || archiving()) return;
    setArchiving(true);
    // bug C · 允许归档"当前话题"：先把玄女切回 general，再归档目标。
    // 旧逻辑直接拒（"先切到其他话题"），用户新建测试话题后想立刻归档完全做不到。
    const wasCurrent = target.id === currentTopicId();
    try {
      if (wasCurrent) {
        await handleSwitch(GENERAL_TOPIC_ID);
      }
      await client.archiveTopic(target.id);
      await refetch();
      setArchiveTarget(null);
    } catch (err) {
      const msg =
        err instanceof ApiError ? `归档失败 (${err.status})` :
        err instanceof Error ? err.message : "归档失败";
      pushToast(msg, "error");
    } finally {
      setArchiving(false);
    }
  };

  // 点抽屉外侧（scrim）关抽屉——仅移动端 sidebarOpen=true 时渲染。
  const closeDrawer = (): void => setSidebarOpen(false);

  // ESC 关抽屉——移动端 a11y。
  onMount(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape" && sidebarOpen()) setSidebarOpen(false);
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  return (
    <>
      {/* 移动端 scrim—— sidebarOpen=true 时绝对覆盖、点击关掉。桌面 CSS 不显。 */}
      <Show when={sidebarOpen()}>
        <div
          class={styles.scrim}
          onClick={closeDrawer}
          data-testid="topic-sidebar-scrim"
          aria-hidden="true"
        />
      </Show>
      <aside
        class={styles.sidebar}
        classList={{ [styles.sidebarOpen ?? ""]: sidebarOpen() }}
        data-testid="topic-sidebar"
        aria-label="话题列表"
      >
        <header class={styles.head}>
          <span class={styles.headTitle}>话题</span>
          <button
            type="button"
            class={styles.addBtn}
            onClick={() => setCreateOpen(true)}
            disabled={creating()}
            aria-label="新建话题"
            data-testid="topic-add-btn"
          >
            +
          </button>
        </header>
        <div class={styles.list} data-testid="topic-list">
          <Show
            when={!data.loading || (data() && topics().length > 0)}
            fallback={
              <p class={styles.muted} data-testid="topic-loading">
                加载中…
              </p>
            }
          >
            <Show when={data.error}>
              <p class={styles.err} role="alert">
                加载失败：{String(data.error)}
              </p>
            </Show>
            <For each={topics()}>
              {(t) => (
                <TopicRow
                  topic={t}
                  current={t.id === currentTopicId()}
                  switching={switching() === t.id}
                  anySwitching={switching() !== null}
                  menuOpen={menuOpenFor() === t.id}
                  onSelect={() => void handleSwitch(t.id)}
                  onMenuToggle={() =>
                    setMenuOpenFor((cur) => (cur === t.id ? null : t.id))
                  }
                  onArchive={() => void handleArchive(t.id, t.title)}
                />
              )}
            </For>
            <Show when={!data.loading && topics().length === 0 && !data.error}>
              <p class={styles.empty} data-testid="topic-empty">
                暂无话题
              </p>
            </Show>
          </Show>
        </div>
      </aside>
      {/* bug D · IM 风格新建 modal —— 取代 window.prompt 的浏览器原生弹窗 */}
      <Show when={createOpen()}>
        <CreateTopicModal
          submitting={creating()}
          onCancel={() => setCreateOpen(false)}
          onSubmit={(title) => void submitCreate(title)}
        />
      </Show>
      {/* 归档确认 modal —— 取代 window.confirm，跟新建 modal 同风格 */}
      <Show when={archiveTarget()}>
        {(target) => (
          <ArchiveTopicModal
            title={target().title}
            isCurrent={target().id === currentTopicId()}
            submitting={archiving()}
            onCancel={() => {
              if (!archiving()) setArchiveTarget(null);
            }}
            onConfirm={() => void confirmArchive()}
          />
        )}
      </Show>
    </>
  );
};

interface CreateTopicModalProps {
  submitting: boolean;
  onCancel(): void;
  onSubmit(title: string): void;
}

const CreateTopicModal: Component<CreateTopicModalProps> = (props) => {
  const [title, setTitle] = createSignal("");
  let inputEl: HTMLInputElement | undefined;
  // 长度上限同后端 / submitCreate 校验。char count 用 Array.from 数 grapheme，
  // 让 emoji/汉字一字一格而非 utf-16 unit。
  const len = createMemo(() => [...title()].length);
  const overflow = createMemo(() => len() > 80);

  onMount(() => {
    // 自动聚焦输入框——modal 打开第一时间能打字。
    queueMicrotask(() => inputEl?.focus());
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") props.onCancel();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  const submit = (): void => {
    if (props.submitting) return;
    if (overflow()) return;
    if (title().trim() === "") return;
    props.onSubmit(title());
  };

  return (
    <div
      class={styles.modalScrim}
      data-testid="topic-create-modal-scrim"
      onClick={(e) => {
        if (e.target === e.currentTarget && !props.submitting) props.onCancel();
      }}
    >
      <div
        class={styles.modalCard}
        role="dialog"
        aria-label="新建话题"
        data-testid="topic-create-modal"
      >
        <header class={styles.modalHead}>
          <h2 class={styles.modalTitle}>新建话题</h2>
        </header>
        <p class={styles.modalHint}>给这次对话起个名字（≤80 字）</p>
        <input
          ref={inputEl}
          type="text"
          class={styles.modalInput}
          classList={{ [styles.modalInputErr ?? ""]: overflow() }}
          placeholder="例：英语 app 调研、周末画画…"
          value={title()}
          onInput={(e) => setTitle(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            }
          }}
          disabled={props.submitting}
          data-testid="topic-create-input"
          aria-invalid={overflow()}
        />
        <div class={styles.modalFooter}>
          <span class={styles.modalCount} aria-live="polite">
            {len()}/80
          </span>
          <div class={styles.modalBtnRow}>
            <button
              type="button"
              class={styles.modalCancelBtn}
              onClick={() => props.onCancel()}
              disabled={props.submitting}
              data-testid="topic-create-cancel"
            >
              取消
            </button>
            <button
              type="button"
              class={styles.modalSubmitBtn}
              onClick={submit}
              disabled={props.submitting || title().trim() === "" || overflow()}
              data-testid="topic-create-submit"
            >
              {props.submitting ? "建中…" : "创建"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

interface ArchiveTopicModalProps {
  title: string;
  /** 是否归档当前正在用的 topic——会附加"先切回 general"提示 */
  isCurrent: boolean;
  submitting: boolean;
  onCancel(): void;
  onConfirm(): void;
}

const ArchiveTopicModal: Component<ArchiveTopicModalProps> = (props) => {
  onMount(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape" && !props.submitting) props.onCancel();
      if (e.key === "Enter" && !props.submitting) props.onConfirm();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
  return (
    <div
      class={styles.modalScrim}
      data-testid="topic-archive-modal-scrim"
      onClick={(e) => {
        if (e.target === e.currentTarget && !props.submitting) props.onCancel();
      }}
    >
      <div
        class={styles.modalCard}
        role="dialog"
        aria-label="归档话题"
        data-testid="topic-archive-modal"
      >
        <header class={styles.modalHead}>
          <h2 class={styles.modalTitle}>归档话题</h2>
        </header>
        <p class={styles.modalHint}>
          归档不删——消息保留，sidebar 默认不显，可在数据库手动恢复。
        </p>
        <p class={styles.modalTarget} data-testid="topic-archive-target">
          「{props.title}」
        </p>
        <Show when={props.isCurrent}>
          <p class={styles.modalWarn}>这是当前话题，归档前会先切回 general。</p>
        </Show>
        <div class={styles.modalFooter}>
          <span aria-hidden="true" />
          <div class={styles.modalBtnRow}>
            <button
              type="button"
              class={styles.modalCancelBtn}
              onClick={() => props.onCancel()}
              disabled={props.submitting}
              data-testid="topic-archive-cancel"
            >
              取消
            </button>
            <button
              type="button"
              class={styles.modalDangerBtn}
              onClick={() => props.onConfirm()}
              disabled={props.submitting}
              data-testid="topic-archive-confirm"
            >
              {props.submitting ? "归档中…" : "归档"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

interface TopicRowProps {
  topic: TopicView;
  current: boolean;
  switching: boolean;
  anySwitching: boolean;
  menuOpen: boolean;
  onSelect(): void;
  onMenuToggle(): void;
  onArchive(): void;
}

const TopicRow: Component<TopicRowProps> = (props) => {
  return (
    <div
      class={styles.row}
      classList={{
        [styles.rowCurrent ?? ""]: props.current,
        [styles.rowDisabled ?? ""]: props.anySwitching && !props.switching,
      }}
      data-testid={`topic-row-${props.topic.id}`}
      data-current={props.current ? "true" : "false"}
    >
      <button
        type="button"
        class={styles.rowMain}
        onClick={() => props.onSelect()}
        disabled={props.anySwitching}
        aria-current={props.current ? "true" : "false"}
        aria-label={
          props.current
            ? `当前话题 ${props.topic.title}`
            : `切到话题 ${props.topic.title}`
        }
      >
        <span class={styles.rowDot} aria-hidden="true" />
        <span class={styles.rowTitle}>{props.topic.title}</span>
        <Show when={props.switching}>
          <span
            class={styles.spinner}
            aria-hidden="true"
            data-testid={`topic-spinner-${props.topic.id}`}
          />
        </Show>
      </button>
      <div class={styles.rowMenuWrap}>
        <button
          type="button"
          class={styles.rowMenuBtn}
          onClick={(e) => {
            e.stopPropagation();
            props.onMenuToggle();
          }}
          aria-label={`${props.topic.title} 的操作菜单`}
          data-testid={`topic-menu-btn-${props.topic.id}`}
        >
          ⋯
        </button>
        <Show when={props.menuOpen}>
          <div
            class={styles.rowMenu}
            role="menu"
            data-testid={`topic-menu-${props.topic.id}`}
          >
            <button
              type="button"
              class={styles.rowMenuItem}
              role="menuitem"
              onClick={(e) => {
                e.stopPropagation();
                props.onArchive();
              }}
              data-testid={`topic-archive-${props.topic.id}`}
            >
              归档
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
};
