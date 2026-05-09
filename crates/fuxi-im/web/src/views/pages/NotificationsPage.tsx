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
import type { FixRef, IssueEvent, IssueStatus, NotificationView } from "~/types/api";
import styles from "./NotificationsPage.module.css";

// 通知 tab · v1-session19 · GitHub-issue 化
//
// 拆 2 子 tab：
//   - 「Issues」（kind == "bug"）：玄女上报 + Claude link-fix → awaiting_test → user/玄女测试关闭
//     状态 filter：active(默认 open+awaiting_test) / open / awaiting_test / closed / all
//     操作：close / reopen（玄女 + 用户都可）
//     展开：body + fix commits + events 时间线
//   - 「其他」（kind != "bug"）：review_request / context_handoff_offer / system / agent_dead 等
//     行为同老的「通知」tab——单条 close 即可
//
// 进 tab 自动 mark_all_read。后台 15s 轮询。

const POLL_MS = 15000;

type SubTab = "issues" | "other";
type IssueFilter = "active" | "open" | "awaiting_test" | "closed" | "all";

export const NotificationsPage: Component = () => {
  const { client } = useApi();
  const [subTab, setSubTab] = createSignal<SubTab>("issues");
  const [issueFilter, setIssueFilter] = createSignal<IssueFilter>("active");
  const [otherIncludeClosed, setOtherIncludeClosed] = createSignal(false);
  const [busy, setBusy] = createSignal(false);

  // Issues tab 数据源
  const [issuesData, { refetch: refetchIssues }] = createResource(
    () => issueFilter(),
    (f) => {
      const opts: { kind: string; status?: string; include_closed?: boolean; limit: number } = {
        kind: "bug",
        limit: 200,
      };
      if (f === "open" || f === "awaiting_test" || f === "closed") {
        opts.status = f;
      } else if (f === "all") {
        opts.include_closed = true;
      }
      // active = 默认 status filter 不传 + include_closed=false → open+awaiting_test
      return client.fetchNotifications(opts);
    },
  );

  // 「其他」tab 数据源（kind != bug；后端没法直接 NOT IN 过滤，前端取全集再排除 bug）
  const [otherData, { refetch: refetchOther }] = createResource(
    () => otherIncludeClosed(),
    (incl) => client.fetchNotifications({ include_closed: incl, limit: 200 }),
  );

  onMount(() => {
    void client.readAllNotifications().catch(() => {});
    const t = setInterval(() => {
      void refetchIssues();
      void refetchOther();
    }, POLL_MS);
    onCleanup(() => clearInterval(t));
  });

  const onClose = async (id: string, actor: string = "user", note?: string): Promise<void> => {
    setBusy(true);
    try {
      await client.closeNotification(id, { actor, note });
      await refetchIssues();
      await refetchOther();
    } finally {
      setBusy(false);
    }
  };

  const onReopen = async (id: string, actor: string = "user", note?: string): Promise<void> => {
    setBusy(true);
    try {
      await client.reopenNotification(id, { actor, note });
      await refetchIssues();
    } finally {
      setBusy(false);
    }
  };

  const issues = createMemo<NotificationView[]>(() => issuesData()?.notifications ?? []);
  const otherList = createMemo<NotificationView[]>(() =>
    (otherData()?.notifications ?? []).filter((n) => n.kind !== "bug"),
  );

  return (
    <div class={styles.page} data-testid="page-notifications">
      <header class={styles.header}>
        <h1 class={styles.title}>通知</h1>
        <div class={styles.subTabs} role="tablist">
          <button
            type="button"
            role="tab"
            class={styles.subTab}
            classList={{ [styles.subTabActive ?? ""]: subTab() === "issues" }}
            onClick={() => setSubTab("issues")}
            data-testid="subtab-issues"
          >
            Issues
          </button>
          <button
            type="button"
            role="tab"
            class={styles.subTab}
            classList={{ [styles.subTabActive ?? ""]: subTab() === "other" }}
            onClick={() => setSubTab("other")}
            data-testid="subtab-other"
          >
            其他
          </button>
        </div>
      </header>

      <Show when={subTab() === "issues"}>
        <div class={styles.filterBar}>
          <For
            each={[
              { v: "active" as IssueFilter, label: "进行中" },
              { v: "open" as IssueFilter, label: "待修" },
              { v: "awaiting_test" as IssueFilter, label: "待测试" },
              { v: "closed" as IssueFilter, label: "已关闭" },
              { v: "all" as IssueFilter, label: "全部" },
            ]}
          >
            {(item) => (
              <button
                type="button"
                class={styles.filterChip}
                classList={{ [styles.filterChipActive ?? ""]: issueFilter() === item.v }}
                onClick={() => setIssueFilter(item.v)}
                data-testid={`issue-filter-${item.v}`}
              >
                {item.label}
              </button>
            )}
          </For>
        </div>
      </Show>

      <Show when={subTab() === "other"}>
        <div class={styles.filterBar}>
          <label class={styles.toggle}>
            <input
              type="checkbox"
              checked={otherIncludeClosed()}
              onChange={(e) => setOtherIncludeClosed(e.currentTarget.checked)}
              data-testid="other-include-closed"
            />
            <span>显已关闭</span>
          </label>
        </div>
      </Show>

      <div class={styles.body}>
        <Show when={subTab() === "issues"}>
          <Show
            when={!issuesData.loading || issuesData()}
            fallback={<p class={styles.muted}>加载中…</p>}
          >
            <Show
              when={issues().length > 0}
              fallback={
                <div class={styles.empty} data-testid="issues-empty">
                  <p class={styles.emptyTitle}>没有 issue</p>
                  <p class={styles.emptyHint}>
                    玄女撞到 fuxi 平台 bug 时会上报；Claude 修完会自动转「待测试」
                  </p>
                </div>
              }
            >
              <ul class={styles.list}>
                <For each={issues()}>
                  {(n) => (
                    <IssueCard
                      n={n}
                      busy={busy()}
                      onClose={(note) => onClose(n.id, "user", note)}
                      onReopen={(note) => onReopen(n.id, "user", note)}
                    />
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </Show>

        <Show when={subTab() === "other"}>
          <Show
            when={!otherData.loading || otherData()}
            fallback={<p class={styles.muted}>加载中…</p>}
          >
            <Show
              when={otherList().length > 0}
              fallback={
                <div class={styles.empty} data-testid="other-empty">
                  <p class={styles.emptyTitle}>暂无其他通知</p>
                  <p class={styles.emptyHint}>
                    门客等审阅 / 玄女 handoff offer / 系统提示 时这里会有红点
                  </p>
                </div>
              }
            >
              <ul class={styles.list}>
                <For each={otherList()}>
                  {(n) => (
                    <SimpleNotificationCard
                      n={n}
                      busy={busy()}
                      onClose={() => onClose(n.id, "user")}
                    />
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
};

// ─── Issue 卡片（GitHub-issue 风格：状态 badge + 展开时间线 + 操作）────

const IssueCard: Component<{
  n: NotificationView;
  busy: boolean;
  onClose: (note?: string) => void;
  onReopen: (note?: string) => void;
}> = (props) => {
  const [expanded, setExpanded] = createSignal(false);
  const status = (): IssueStatus => (props.n.status as IssueStatus) ?? "open";
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
  const statusBadge = (): { label: string; className: string } => {
    switch (status()) {
      case "awaiting_test":
        return { label: "待测试", className: styles.statusAwaiting ?? "" };
      case "closed":
        return { label: "已关闭", className: styles.statusClosed ?? "" };
      default:
        return { label: "待修", className: styles.statusOpen ?? "" };
    }
  };
  const fixRefs = (): FixRef[] => props.n.fix_refs ?? [];
  const events = (): IssueEvent[] => props.n.events ?? [];

  return (
    <li
      class={styles.card}
      classList={{
        [styles.cardClosed ?? ""]: status() === "closed",
        [styles.cardAwaiting ?? ""]: status() === "awaiting_test",
      }}
      data-testid={`issue-${props.n.id}`}
      data-status={status()}
      data-severity={props.n.severity}
    >
      <div
        class={styles.cardRow}
        onClick={() => setExpanded(!expanded())}
        role="button"
        tabIndex={0}
      >
        <span class={`${styles.sevDot} ${sevClass()}`} aria-hidden="true" />
        <span class={`${styles.statusBadge} ${statusBadge().className}`}>
          {statusBadge().label}
        </span>
        <span class={styles.cardTitle}>{props.n.title}</span>
        <Show when={fixRefs().length > 0}>
          <span class={styles.fixCount} aria-label="fix 数">
            ✓{fixRefs().length}
          </span>
        </Show>
      </div>

      <div class={styles.meta}>
        <time>{formatTs(props.n.created_at)}</time>
        <Show when={props.n.task_id}>
          {(tid) => <span class={styles.metaSep}>· task {shortUuid(tid())}</span>}
        </Show>
      </div>

      <Show when={expanded()}>
        <Show when={props.n.body && props.n.body.trim().length > 0}>
          <div class={styles.detailBody}>{props.n.body}</div>
        </Show>

        <Show when={fixRefs().length > 0}>
          <div class={styles.section}>
            <div class={styles.sectionTitle}>fix commits ({fixRefs().length})</div>
            <ul class={styles.fixList}>
              <For each={fixRefs()}>
                {(f) => (
                  <li class={styles.fixItem}>
                    <code class={styles.commitSha}>{f.commit_sha}</code>
                    <Show when={f.branch}>
                      {(b) => <span class={styles.branchTag}>{b()}</span>}
                    </Show>
                    <Show when={f.summary}>
                      {(s) => <span class={styles.fixSummary}>{s()}</span>}
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Show>

        <Show when={events().length > 0}>
          <div class={styles.section}>
            <div class={styles.sectionTitle}>events ({events().length})</div>
            <ul class={styles.eventList}>
              <For each={events()}>
                {(e) => (
                  <li class={styles.eventItem}>
                    <time class={styles.eventTime}>{formatTs(e.at)}</time>
                    <span class={styles.eventActor}>{e.actor}</span>
                    <span class={styles.eventAction}>{actionLabel(e.action)}</span>
                    <Show when={e.from && e.to && e.action !== "created"}>
                      <span class={styles.eventTransition}>
                        {e.from} → {e.to}
                      </span>
                    </Show>
                    <Show when={e.note}>
                      {(note) => <span class={styles.eventNote}>// {note()}</span>}
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Show>

        <div class={styles.actions}>
          <Show when={status() !== "closed"}>
            <button
              type="button"
              class={styles.actionPrimary}
              disabled={props.busy}
              onClick={(e) => {
                e.stopPropagation();
                props.onClose();
              }}
              data-testid={`issue-${props.n.id}-close`}
            >
              {status() === "awaiting_test" ? "测过了 → 关闭" : "我关掉"}
            </button>
          </Show>
          <Show when={status() === "closed"}>
            <button
              type="button"
              class={styles.actionSecondary}
              disabled={props.busy}
              onClick={(e) => {
                e.stopPropagation();
                props.onReopen();
              }}
              data-testid={`issue-${props.n.id}-reopen`}
            >
              重新打开
            </button>
          </Show>
        </div>
      </Show>
    </li>
  );
};

// ─── 简单通知卡片（kind != bug 的旧路径——review/handoff/system）────

const SimpleNotificationCard: Component<{
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
        <p class={styles.simpleBody}>{props.n.body}</p>
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

function actionLabel(a: string): string {
  switch (a) {
    case "created":
      return "上报";
    case "status_changed":
      return "状态变更";
    case "fix_linked":
      return "关联 fix";
    case "closed":
      return "关闭";
    case "reopened":
      return "重开";
    default:
      return a;
  }
}

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
