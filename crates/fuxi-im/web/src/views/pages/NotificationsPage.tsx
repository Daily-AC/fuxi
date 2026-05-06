import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import type { NotificationView } from "~/types/api";
import styles from "./NotificationsPage.module.css";

// 通知 tab · v1-session16
//
// 用途：聚集"等用户消费"的事项，区别于玄女 tab 双向对话。
// kind:
//   - bug: 玄女自报 fuxi 平台 bug
//   - review_request: 门客交付等审阅（task #8 后通用化）
//   - context_handoff_offer: 玄女 context 45% 询问换副本（task #8）
//   - system: 平台级
//
// 行为：
//   - 进入 tab 自动 mark_all_read（红点清零）；列表里仍可见
//   - tap 单条 → 展开 body；右滑 / × 按钮 → 关闭（默认列表隐藏）
//   - 「显已关闭」toggle → list 加 ?include_closed=true

const POLL_MS = 15000;

export const NotificationsPage: Component = () => {
  const { client } = useApi();
  const [includeClosed, setIncludeClosed] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [data, { refetch }] = createResource(
    () => includeClosed(),
    (incl) => client.fetchNotifications({ include_closed: incl }),
  );

  onMount(() => {
    // 进 tab 一刹那把所有未读标 read（红点清零）。failure 不致命。
    void client.readAllNotifications().catch(() => {});
    // 后台轮询拉新（不接 WS，简单起步）。
    const t = setInterval(() => {
      void refetch();
    }, POLL_MS);
    onCleanup(() => clearInterval(t));
  });

  const onClose = async (id: string): Promise<void> => {
    setBusy(true);
    try {
      await client.closeNotification(id);
      await refetch();
    } finally {
      setBusy(false);
    }
  };

  const list = createMemo<NotificationView[]>(() => data()?.notifications ?? []);

  return (
    <div class={styles.page} data-testid="page-notifications">
      <header class={styles.header}>
        <h1 class={styles.title}>通知</h1>
        <label class={styles.toggle}>
          <input
            type="checkbox"
            checked={includeClosed()}
            onChange={(e) => setIncludeClosed(e.currentTarget.checked)}
            data-testid="notifications-include-closed"
          />
          <span>显已关闭</span>
        </label>
      </header>
      <div class={styles.body}>
        <Show
          when={!data.loading || data()}
          fallback={<p class={styles.muted}>加载中…</p>}
        >
          <Show
            when={list().length > 0}
            fallback={
              <div class={styles.empty} data-testid="notifications-empty">
                <p class={styles.emptyTitle}>暂无通知</p>
                <p class={styles.emptyHint}>
                  玄女撞到 bug / 门客等审阅 时这里会有红点
                </p>
              </div>
            }
          >
            <ul class={styles.list}>
              <For each={list()}>
                {(n) => (
                  <NotificationCard
                    n={n}
                    busy={busy()}
                    onClose={() => onClose(n.id)}
                  />
                )}
              </For>
            </ul>
          </Show>
        </Show>
      </div>
    </div>
  );
};

const NotificationCard: Component<{
  n: NotificationView;
  busy: boolean;
  onClose: () => void;
}> = (props) => {
  const sevClass = (): string => {
    switch (props.n.severity) {
      case "error":
        return styles.sevError ?? "";
      case "warn":
        return styles.sevWarn ?? "";
      default:
        return styles.sevInfo ?? "";
    }
  };
  const kindLabel = (): string => {
    switch (props.n.kind) {
      case "bug":
        return "BUG";
      case "review_request":
        return "审阅";
      case "context_handoff_offer":
        return "玄女";
      case "system":
        return "系统";
      default:
        return props.n.kind;
    }
  };
  const isClosed = (): boolean => Boolean(props.n.closed_at);
  return (
    <li
      class={styles.card}
      classList={{ [styles.cardClosed ?? ""]: isClosed() }}
      data-testid={`notification-${props.n.id}`}
      data-kind={props.n.kind}
      data-severity={props.n.severity}
    >
      <div class={styles.cardRow}>
        <span class={`${styles.sevDot} ${sevClass()}`} aria-hidden="true" />
        <span class={styles.kind}>{kindLabel()}</span>
        <span class={styles.cardTitle}>{props.n.title}</span>
        <Show when={!isClosed()}>
          <button
            type="button"
            class={styles.closeBtn}
            onClick={props.onClose}
            disabled={props.busy}
            data-testid={`notification-${props.n.id}-close`}
            aria-label={`关闭 ${props.n.title}`}
          >
            ×
          </button>
        </Show>
      </div>
      <Show when={props.n.body && props.n.body.trim().length > 0}>
        <p class={styles.body}>{props.n.body}</p>
      </Show>
      <div class={styles.meta}>
        <time>{formatTs(props.n.created_at)}</time>
        <Show when={props.n.task_id}>
          {(tid) => <span class={styles.metaSep}>· task {shortUuid(tid())}</span>}
        </Show>
        <Show when={isClosed()}>
          <span class={styles.metaSep}>· 已关闭</span>
        </Show>
      </div>
    </li>
  );
};

function formatTs(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const now = Date.now();
  const diff = now - d.getTime();
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  return d.toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
}

function shortUuid(s: string): string {
  const trimmed = s.startsWith("task-") ? s.slice(5) : s;
  return trimmed.slice(0, 8);
}
