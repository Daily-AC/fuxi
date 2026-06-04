import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  type Accessor,
  type Component,
  createEffect,
  on,
} from "solid-js";
import { ApiError } from "~/lib/api";
import { startReconnectingSocket, type ReconnectController } from "~/lib/reconnect";
import { pushToast } from "~/lib/toast";
import {
  applyWorkerEvent,
  groupConsecutiveToolCalls,
  makeUserMessage,
  markUserMessage,
  mergeMessages,
  type Message,
  type WorkerReducerCtx,
} from "~/messages";
import type { ServerEvent } from "~/types/events";
import type { TasksOverview, Upload } from "~/types/api";
import { useApi } from "~/components/ApiProvider";
import { Composer } from "~/components/Composer";
import { UserBubble } from "~/components/messages/UserBubble";
import { WorkerBubble } from "~/components/messages/WorkerBubble";
import { InlineFileCard } from "~/components/messages/InlineFileCard";
import { ToolCallCard } from "~/components/messages/ToolCallCard";
import { ToolGroupCard } from "~/components/messages/ToolGroupCard";
import { ThinkingRow } from "~/components/messages/ThinkingRow";
import { StatusMarkerRow } from "~/components/messages/StatusMarkerRow";
import { SystemMessageRow } from "~/components/messages/SystemMessageRow";
import { formatDuration } from "~/lib/format-task";
import { Card } from "~/components/ui/Card";
import { StatePill, type StatePillTone } from "~/components/ui/StatePill";
import { EmptyState } from "~/components/ui/EmptyState";
import styles from "./WorkerPage.module.css";

// 私聊页 modal · #N3 · spec docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §C
//
// 数据源：
//   GET /api/workers/:agent_id/events  (β #N5 后端镜像)
//   WS  /api/workers/:agent_id/conv    (β #N5)
// 后端 #N5 落地前 mock api 已支持 pushWorker / fetchWorkerEvents。
//
// composer 走 client.intervene({ target: agent_id, text, attachments })
// 4xx → toast "门客正忙，等这轮跑完再发"；统一文案不区分 cc/codex 适配器（spec §私聊页·composer）。

const RETRY_DELAY_MS = 1500;

export interface WorkerPageProps {
  agent_id: string;
  role_display?: string;
}

export const WorkerPage: Component<WorkerPageProps> = (props) => {
  const { client, navPop } = useApi();

  const [messages, setMessages] = createSignal<Message[]>([]);
  const [online, setOnline] = createSignal(false);

  // task 上下文 banner · 从 /api/tasks 找该 agent 当前所属 running task（v1 取第一条）
  // 后续如果一个 worker 跨多 task，sheet 选择来源会传 task_id 进来；v1 简化。
  const [overview] = createResource(() => client.fetchTasksOverview());

  const taskCtx = createMemo<{ id: string; title: string; status: string; duration_ms: number } | null>(
    () => pickTaskFor(overview(), props.agent_id),
  );

  let controller: ReconnectController | null = null;

  const handleEvent = (ev: ServerEvent, snap: WorkerReducerCtx): void => {
    setMessages((prev) => applyWorkerEvent(prev, ev, snap));
  };

  // agent_id 变化时重新加载（v1 实际上只 mount 一次，但保险起见用 effect）
  // effect 内 snapshot props 到 local 让 lint 看到 reactivity 边界。
  createEffect(
    on(
      () => props.agent_id,
      (agentId) => {
        const snap: WorkerReducerCtx = { agent: agentId, role_display: props.role_display };
        setMessages([]);
        controller?.dispose();
        void (async () => {
          try {
            const r = await client.fetchWorkerEvents(agentId);
            const seeded = r.events.reduce<Message[]>(
              (acc, ev) => applyWorkerEvent(acc, ev, snap),
              [],
            );
            if (seeded.length > 0) setMessages((prev) => mergeMessages(prev, seeded));
          } catch (err) {
            console.warn("worker history load failed", err);
          }
        })();
        controller = startReconnectingSocket(
          () => client.openWorkerSocket(agentId),
          {
            onOpen: () => setOnline(true),
            onClose: () => setOnline(false),
            onError: () => setOnline(false),
            onMessage: (e) => {
              try {
                const ev = JSON.parse(e.data) as ServerEvent;
                handleEvent(ev, snap);
              } catch (err) {
                console.warn("worker event parse failed", err);
              }
            },
            // 切回前台 → refetch 私聊页历史补漏，mergeMessages 按 id 去重。
            onVisible: () => {
              void (async () => {
                try {
                  const r = await client.fetchWorkerEvents(agentId);
                  const seeded = r.events.reduce<Message[]>(
                    (acc, ev) => applyWorkerEvent(acc, ev, snap),
                    [],
                  );
                  if (seeded.length > 0)
                    setMessages((prev) => mergeMessages(prev, seeded));
                } catch (err) {
                  console.warn("worker visibility refetch failed", err);
                }
              })();
            },
          },
        );
      },
    ),
  );

  // 顶栏 ESC / 浏览器返回 · NavigationStack 已接管 ESC 和 edge swipe，这里不重复

  onCleanup(() => {
    controller?.dispose();
    controller = null;
  });

  const sendIntervene = async (
    text: string,
    attachmentIds: string[],
    msgId: string,
  ): Promise<void> => {
    const send = (): Promise<unknown> =>
      client.intervene({
        text,
        task_id: null,
        attachments: attachmentIds,
        target: props.agent_id,
      });
    try {
      await send();
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: null }));
      return;
    } catch (err) {
      // 4xx · spec 统一文案：门客正忙
      if (err instanceof ApiError && err.status >= 400 && err.status < 500) {
        pushToast("门客正忙，等这轮跑完再发", "error");
        setMessages((prev) =>
          markUserMessage(prev, msgId, { pending: false, error: "门客正忙" }),
        );
        return;
      }
      // 503 重试一次（跟玄女主对话同策略）
      if (err instanceof ApiError && err.status === 503) {
        await new Promise((r) => setTimeout(r, RETRY_DELAY_MS));
        try {
          await send();
          setMessages((prev) =>
            markUserMessage(prev, msgId, { pending: false, error: null }),
          );
          return;
        } catch (err2) {
          const msg =
            err2 instanceof ApiError && err2.status === 503
              ? "服务暂忙，稍后再试"
              : err2 instanceof Error
                ? err2.message
                : "发送失败";
          pushToast(msg, "error");
          setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: msg }));
          return;
        }
      }
      const fallback = err instanceof Error ? err.message : "发送失败";
      pushToast(fallback, "error");
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: fallback }));
    }
  };

  const handleSubmit = async (text: string, attachments: Upload[]): Promise<void> => {
    const m = makeUserMessage(text, attachments.length > 0 ? attachments : undefined);
    setMessages((prev) => [...prev, m]);
    void sendIntervene(
      text,
      attachments.map((u) => u.id),
      m.id,
    );
  };

  const placeholder = (): string => `跟${props.role_display ?? "门客"}说……`;

  return (
    <div class={styles.page} data-testid="page-worker" data-agent-id={props.agent_id}>
      <header class={styles.topbar}>
        <button
          type="button"
          class={styles.back}
          onClick={() => navPop()}
          data-testid="worker-back"
          aria-label="返回玄女"
        >
          <svg
            class={styles.backIcon}
            width="9"
            height="15"
            viewBox="0 0 9 15"
            fill="none"
            aria-hidden="true"
          >
            <path
              d="M7.5 1.5 1.5 7.5l6 6"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
          玄女
        </button>
        <div class={styles.titleWrap}>
          <span class={styles.role}>{props.role_display ?? "门客"}</span>
          <span class={`${styles.agentId} agent-id`} aria-hidden="true">
            {shortAgent(props.agent_id)}
          </span>
        </div>
        <button
          type="button"
          class={styles.more}
          aria-label="更多"
          data-testid="worker-more"
          disabled
        >
          <svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true">
            <circle cx="4" cy="10" r="1.6" fill="currentColor" />
            <circle cx="10" cy="10" r="1.6" fill="currentColor" />
            <circle cx="16" cy="10" r="1.6" fill="currentColor" />
          </svg>
        </button>
      </header>

      <Show when={taskCtx()}>
        {(t) => (
          <div class={styles.bannerWrap}>
            <Card tone="peach" class={styles.banner}>
              <div data-testid="worker-task-banner" class={styles.bannerInner}>
                <span class={styles.bannerLabel}>任务</span>
                <span class={styles.bannerTitle} title={t().title}>
                  {t().title}
                </span>
                <span class={styles.bannerMeta}>{formatDuration(t().duration_ms)}</span>
                <StatePill
                  label={bannerStatusLabel(t().status)}
                  tone={bannerStatusTone(t().status)}
                />
              </div>
            </Card>
          </div>
        )}
      </Show>

      <Thread messages={messages} role_display={props.role_display} online={online} />

      <div class={styles.composerWrap}>
        <Composer onSubmit={handleSubmit} placeholder={placeholder()} />
      </div>
    </div>
  );
};

interface ThreadProps {
  messages: Accessor<Message[]>;
  role_display?: string;
  online: Accessor<boolean>;
}

const STICK_THRESHOLD = 80;

const Thread: Component<ThreadProps> = (props) => {
  let scrollEl: HTMLDivElement | undefined;
  const [stick, setStick] = createSignal(true);

  const isAtBottom = (): boolean => {
    if (!scrollEl) return true;
    const slack = scrollEl.scrollHeight - scrollEl.clientHeight - scrollEl.scrollTop;
    return slack < STICK_THRESHOLD;
  };

  onMount(() => {
    if (!scrollEl) return;
    scrollEl.addEventListener("scroll", () => setStick(isAtBottom()));
  });

  createEffect(
    on(
      () => {
        const list = props.messages();
        const last = list[list.length - 1];
        const lastText = last && "text" in last ? (last.text ?? "").length : 0;
        return [list.length, lastText] as const;
      },
      () => {
        if (!stick() || !scrollEl) return;
        queueMicrotask(() => {
          scrollEl?.scrollTo({ top: scrollEl.scrollHeight, behavior: "smooth" });
        });
      },
      { defer: true },
    ),
  );

  return (
    <section ref={scrollEl} class={`u-mesh ${styles.thread}`} data-testid="worker-thread">
      <Show
        when={props.messages().length > 0}
        fallback={
          <div class={styles.empty} data-testid="worker-empty">
            <EmptyState
              mascotState={props.online() ? "idle" : "sleep"}
              title={`${props.role_display ?? "门客"}在线`}
              hint={props.online() ? "跟他/她说点啥" : "重连中…"}
            />
          </div>
        }
      >
        <For each={groupConsecutiveToolCalls(props.messages())}>
          {(msg) => {
            if (msg.kind === "user") {
              return (
                <div class={styles.rowRight}>
                  <UserBubble msg={msg} />
                </div>
              );
            }
            if (msg.kind === "worker") {
              return (
                <div class={styles.rowLeft}>
                  <WorkerBubble msg={msg} />
                </div>
              );
            }
            if (msg.kind === "tool_call") return <ToolCallCard msg={msg} />;
            if (msg.kind === "tool_group") {
              return (
                <div class={styles.rowLeft}>
                  <ToolGroupCard items={msg.items} role_display={props.role_display} />
                </div>
              );
            }
            if (msg.kind === "thinking") return <ThinkingRow msg={msg} />;
            if (msg.kind === "marker") return <StatusMarkerRow msg={msg} />;
            // bug #76：bridge 抄送（用户直接对该 worker 说话时）
            if (msg.kind === "system") return <SystemMessageRow msg={msg} />;
            // P2.7 · 门客 inline 文件推送
            if (msg.kind === "inline_file") {
              return (
                <div class={styles.rowLeft}>
                  <InlineFileCard msg={msg} />
                </div>
              );
            }
            return null;
          }}
        </For>
      </Show>
    </section>
  );
};

function pickTaskFor(
  ov: TasksOverview | undefined,
  agentId: string,
): { id: string; title: string; status: string; duration_ms: number } | null {
  if (!ov) return null;
  // 优先 running，再 completed
  for (const t of ov.running) {
    if (t.members.some((m) => m.agent_id === agentId)) {
      return { id: t.id, title: t.title, status: t.status, duration_ms: t.duration_ms };
    }
  }
  for (const t of ov.completed) {
    if (t.members.some((m) => m.agent_id === agentId)) {
      return { id: t.id, title: t.title, status: t.status, duration_ms: t.duration_ms };
    }
  }
  return null;
}

function bannerStatusLabel(status: string): string {
  if (status === "running") return "进行中";
  if (status === "completed") return "已完成";
  if (status === "failed") return "失败";
  return status;
}

function bannerStatusTone(status: string): StatePillTone {
  if (status === "running") return "running";
  if (status === "completed") return "done";
  if (status === "failed") return "danger";
  return "neutral";
}

function shortAgent(id: string): string {
  if (!id) return "";
  return id.length > 12 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}
