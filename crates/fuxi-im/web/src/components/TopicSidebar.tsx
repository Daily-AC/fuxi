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
  } = useApi();

  const [data, { refetch }] = createResource(() => client.fetchTopics(false));
  const [switching, setSwitching] = createSignal<string | null>(null);
  const [creating, setCreating] = createSignal(false);
  const [menuOpenFor, setMenuOpenFor] = createSignal<string | null>(null);

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
    }
  };

  const handleCreate = async (): Promise<void> => {
    if (creating()) return;
    const raw = window.prompt("新话题标题（≤80 字）", "");
    if (raw === null) return; // 用户取消
    const title = raw.trim();
    if (title === "") {
      pushToast("标题不能为空", "warn");
      return;
    }
    if ([...title].length > 80) {
      pushToast("标题上限 80 字", "warn");
      return;
    }
    setCreating(true);
    try {
      const t = await client.createTopic({ title });
      await refetch();
      // 自动 switch 到新建的 topic——用户建 = 想进
      await handleSwitch(t.id);
    } catch (err) {
      const msg =
        err instanceof ApiError ? `建话题失败 (${err.status})` :
        err instanceof Error ? err.message : "建话题失败";
      pushToast(msg, "error");
    } finally {
      setCreating(false);
    }
  };

  const handleArchive = async (id: string, title: string): Promise<void> => {
    setMenuOpenFor(null);
    if (id === currentTopicId()) {
      pushToast("当前话题不可归档，请先切到其他话题", "warn");
      return;
    }
    if (!window.confirm(`归档话题「${title}」？归档不删，可在数据库手动恢复。`)) {
      return;
    }
    try {
      await client.archiveTopic(id);
      await refetch();
    } catch (err) {
      const msg =
        err instanceof ApiError ? `归档失败 (${err.status})` :
        err instanceof Error ? err.message : "归档失败";
      pushToast(msg, "error");
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
            onClick={() => void handleCreate()}
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
    </>
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
