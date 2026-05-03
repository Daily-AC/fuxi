import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  on,
  onCleanup,
  onMount,
  type Accessor,
  type Component,
} from "solid-js";
import { ApiError } from "~/lib/api";
import { startReconnectingSocket, type ReconnectController } from "~/lib/reconnect";
import { pushToast } from "~/lib/toast";
import {
  applyTaskThreadEvent,
  makeUserMessage,
  markUserMessage,
  mergeMessages,
  type Message,
  type TaskThreadCtx,
} from "~/messages";
import type { ServerEvent } from "~/types/events";
import type { TaskGroupCard, TasksOverview } from "~/types/api";
import { useApi } from "~/components/ApiProvider";
import { MentionComposer } from "~/components/MentionComposer";
import { UserBubble } from "~/components/messages/UserBubble";
import { WorkerBubble } from "~/components/messages/WorkerBubble";
import { XuannvBubble } from "~/components/messages/XuannvBubble";
import { ToolCallCard } from "~/components/messages/ToolCallCard";
import { ThinkingRow } from "~/components/messages/ThinkingRow";
import { StatusMarkerRow } from "~/components/messages/StatusMarkerRow";
import {
  candidatesFromMembers,
  candidatesFromNodes,
  sortCandidates,
  type MentionCandidate,
  type SerializedIntervene,
} from "~/lib/mentions";
import { formatDuration } from "~/lib/format-task";
import styles from "./TaskThreadPage.module.css";

// 任务 thread Layer 2 · v3 #N4' / #39
//
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §"任务 tab · Layer 2"
//
// 心智参照微信群聊：所有人在一个 thread，按时间排，按 who 标签区分。
//
// 数据源：
//   - GET /api/tasks/:task_id/events  (β #41 已落，白名单 filter)
//   - WS  /api/tasks/:task_id/conv    (β #41 已落，filter 同上)
//   - GET /api/tasks 任意一次拿成员列表（用于 banner + composer 候选 + bubble 着色）
//
// composer 默认对玄女说（无 chip）；@ 选成员 → chip + 走 target=mentioned。
// 4xx → toast "门客正忙"。

const RETRY_DELAY_MS = 1500;

export interface TaskThreadPageProps {
  task_id: string;
  /** 列表 push 进来时已知的 title，banner 数据 race 前用作 fallback。*/
  fallback_title?: string;
}

export const TaskThreadPage: Component<TaskThreadPageProps> = (props) => {
  const { client, navPop } = useApi();

  const [messages, setMessages] = createSignal<Message[]>([]);
  const [online, setOnline] = createSignal(false);

  // /api/tasks 拿成员列表（banner + composer 候选 + bubble 着色源）
  const [overview] = createResource(() => client.fetchTasksOverview());
  const task = createMemo<TaskGroupCard | null>(() =>
    pickTask(overview(), props.task_id),
  );

  // member lookup table for reducer ctx
  const ctx = createMemo<TaskThreadCtx>(() => {
    const t = task();
    const members: Record<string, { role: string; role_display: string }> = {};
    let xuannv_id: string | undefined;
    if (t) {
      for (const m of t.members) {
        members[m.agent_id] = { role: m.role, role_display: m.role_display };
        if (m.role === "xuannv") xuannv_id = m.agent_id;
      }
    }
    return { members, xuannv_id };
  });

  // composer 候选 = 全任务成员 + dist online 节点（v3 #60 加节点段）
  const [nodesData] = createResource(() => client.fetchNodes());
  const candidates = createMemo<MentionCandidate[]>(() => {
    const t = task();
    const workers = t ? sortCandidates(candidatesFromMembers(t.members)) : [];
    const nodes = nodesData() ? candidatesFromNodes(nodesData()!.nodes) : [];
    return [...workers, ...nodes];
  });
  // v3 #64 · online 节点 id 集合 · banner 用来给 @node 切 dim red（离线）vs muted gray（在线）
  // null = nodesData 未 ready · 下游按"未知态默认在线"处理（避免首帧 race 闪红）
  const onlineNodeIds = createMemo<Set<string> | null>(() => {
    const data = nodesData();
    if (!data) return null;
    return new Set(data.nodes.filter((n) => n.online).map((n) => n.node_id));
  });

  let controller: ReconnectController | null = null;

  const handleEvent = (ev: ServerEvent, snap: TaskThreadCtx): void => {
    setMessages((prev) => applyTaskThreadEvent(prev, ev, snap));
  };

  // history fold 等成员表 ready · 避免 ctx 空时玄女被误判 worker
  // （ctx 来自 createResource(fetchTasksOverview)，首帧未必有数据）
  // task_id 变化重起；ctx 变化重 fold（成员表迟到时 catch up）
  let historyLoaded = false;
  let lastTaskId = "";

  createEffect(
    on(
      () => [props.task_id, ctx()] as const,
      ([taskId, snapCtx]) => {
        const taskChanged = lastTaskId !== taskId;
        if (taskChanged) {
          setMessages([]);
          historyLoaded = false;
          lastTaskId = taskId;
          controller?.dispose();
          controller = startReconnectingSocket(
            () => client.openTaskSocket(taskId),
            {
              onOpen: () => setOnline(true),
              onClose: () => setOnline(false),
              onError: () => setOnline(false),
              onMessage: (e) => {
                try {
                  const ev = JSON.parse(e.data) as ServerEvent;
                  handleEvent(ev, ctx());
                } catch (err) {
                  console.warn("task thread event parse failed", err);
                }
              },
            },
          );
        }
        // 历史 fold 仅一次：等 ctx 有 members（玄女在 task 中）后再做，避免色错
        const hasMembers = Object.keys(snapCtx.members).length > 0;
        if (!historyLoaded && hasMembers) {
          historyLoaded = true;
          void (async () => {
            try {
              const r = await client.fetchTaskEvents(taskId);
              const seeded = r.events.reduce<Message[]>(
                (acc, ev) => applyTaskThreadEvent(acc, ev, snapCtx),
                [],
              );
              if (seeded.length > 0) setMessages((prev) => mergeMessages(prev, seeded));
            } catch (err) {
              console.warn("task thread history load failed", err);
            }
          })();
        }
      },
    ),
  );

  onCleanup(() => {
    controller?.dispose();
    controller = null;
  });

  const sendIntervene = async (req: SerializedIntervene, msgId: string): Promise<void> => {
    // v3 #52 fix · 任务 done 后用户在 thread composer 发"你好"被拒：
    // 旧逻辑无脑挂 task_id，backend 见到 task_id 关联的是 done 任务 → 4xx → toast "门客正忙"误导。
    // 修：task 状态非 running 时不挂 task_id（视为对玄女的普通主对话 intervene）。
    const isRunning = task()?.status === "running";
    const send = (): Promise<unknown> =>
      client.intervene({
        text: req.text,
        task_id: isRunning ? props.task_id : null,
        target: req.target,
        mentions: req.mentions.length > 0 ? req.mentions : undefined,
        attachments: req.attachments,
        pinned_node: req.pinned_node,
      });
    try {
      await send();
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: null }));
      return;
    } catch (err) {
      if (err instanceof ApiError && err.status >= 400 && err.status < 500) {
        // #68 fix · task 已完成（或无 worker target）时 4xx 是玄女 busy，不是门客 busy。
        // 文案要区分：targetIsWorker = 任务 running 且 req.target 是 worker；否则是玄女路径。
        const targetIsWorker = isRunning && Boolean(req.target);
        const toastMsg = targetIsWorker
          ? "门客正忙，等这轮跑完再发"
          : "玄女正忙，稍后再试";
        const inlineErr = targetIsWorker ? "门客正忙" : "玄女正忙";
        pushToast(toastMsg, "error");
        setMessages((prev) =>
          markUserMessage(prev, msgId, { pending: false, error: inlineErr }),
        );
        return;
      }
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

  const handleSubmit = async (req: SerializedIntervene): Promise<void> => {
    // optimistic 附件：composer 里已上传完成，placeholder Upload[] 让用户立即看图
    const ups = req.attachments && req.attachments.length > 0
      ? req.attachments.map((id) => ({
          id,
          name: id,
          mime: "application/octet-stream",
          bytes: 0,
          sha256: "",
        }))
      : undefined;
    const m = makeUserMessage(req.text, ups);
    if (req.mentions.length > 0) {
      m.mentions = req.mentions;
    }
    setMessages((prev) => [...prev, m]);
    await sendIntervene(req, m.id);
  };

  const [menuOpen, setMenuOpen] = createSignal(false);

  return (
    <div class={styles.page} data-testid="task-thread-page" data-task-id={props.task_id}>
      <header class={styles.topbar}>
        <button
          type="button"
          class={styles.back}
          onClick={() => navPop()}
          data-testid="task-thread-back"
          aria-label="返回任务列表"
        >
          ‹ 列表
        </button>
        <span class={styles.title}>{task()?.title ?? props.fallback_title ?? "任务"}</span>
        <button
          type="button"
          class={styles.more}
          aria-label="门客详情"
          data-testid="task-thread-more"
          onClick={() => setMenuOpen(true)}
        >
          ⋯
        </button>
      </header>

      <Show when={task()}>
        {(t) => <Banner task={t()} onlineNodeIds={onlineNodeIds()} />}
      </Show>

      <Thread messages={messages} online={online} />

      <MentionComposer
        candidates={candidates()}
        placeholder="对玄女说... (@ 角色或节点)"
        onSubmit={handleSubmit}
      />

      <Show when={menuOpen() && task()}>
        <MembersMenu
          task={task()!}
          onlineNodeIds={onlineNodeIds()}
          onClose={() => setMenuOpen(false)}
        />
      </Show>
    </div>
  );
};

/** ⋯ 菜单 · 详情页右上角 popover · 显示任务全门客 + last_tool_call + @node。
 *  实测反馈：旧版门客预览嵌在卡片里被用户误点击；现在卡片不渲门客，详情页这里
 *  集中呈现。点击遮罩或关闭按钮关。 */
const MembersMenu: Component<{
  task: TaskGroupCard;
  onlineNodeIds: Set<string> | null;
  onClose(): void;
}> = (props) => {
  const offline = (m: TaskGroupCard["members"][number]): boolean =>
    !!m.node_id
    && props.onlineNodeIds !== null
    && !props.onlineNodeIds.has(m.node_id);

  const memberSub = (m: TaskGroupCard["members"][number]): string => {
    if (m.last_activity) return m.last_activity;
    const tool = toolCallText(m.last_tool_call);
    if (tool) return tool;
    if (m.activity) return m.activity;
    if (props.task.status !== "running") return "已歇";
    return memberStatusFallback(m);
  };

  const statusLabel = (m: TaskGroupCard["members"][number]): string => {
    if (props.task.status !== "running") return "已歇";
    if (m.status === "idle") return "待命";
    if (m.status === "thinking") return "思考中";
    if (m.status === "dead") return "下线";
    return "运行中";
  };

  return (
    <div
      class={styles.menuOverlay}
      data-testid="task-thread-menu"
      onClick={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
    >
      <div class={styles.menuPanel} role="dialog" aria-label="门客详情">
        <p class={styles.menuTitle}>门客（{props.task.members.length}）</p>
        <Show
          when={props.task.members.length > 0}
          fallback={<p class={styles.menuMemberSub}>暂无门客</p>}
        >
          <ul class={styles.menuList}>
            <For each={props.task.members}>
              {(m) => (
                <li class={styles.menuMember} data-testid={`menu-member-${m.agent_id}`}>
                  <div class={styles.menuMemberHead}>
                    <span class={styles.menuMemberRole}>{m.role_display}</span>
                    <span class={styles.menuMemberStatus}>· {statusLabel(m)}</span>
                  </div>
                  <span class={styles.menuMemberSub}>{memberSub(m)}</span>
                  <Show when={m.node_id}>
                    {(nid) => (
                      <span
                        class={`${styles.menuMemberNode} ${offline(m) ? styles.menuMemberNodeOffline ?? "" : ""}`}
                      >
                        @{nid()}
                        {offline(m) ? " · 离线" : ""}
                      </span>
                    )}
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <button
          type="button"
          class={styles.menuClose}
          onClick={() => props.onClose()}
          data-testid="task-thread-menu-close"
        >
          关闭
        </button>
      </div>
    </div>
  );
};

const Banner: Component<{ task: TaskGroupCard; onlineNodeIds: Set<string> | null }> = (props) => {
  const statusLabel = (): string => {
    if (props.task.status === "running") return "进行中";
    if (props.task.status === "completed") return "已完成";
    if (props.task.status === "failed") return "失败";
    return props.task.status;
  };

  const statusDotClass = (): string => {
    if (props.task.status === "running") return styles.statusDotRunning ?? "";
    if (props.task.status === "completed") return styles.statusDotDone ?? "";
    return "";
  };

  // 第二行成员列表 = role · last_tool_call.tool（截 12 字符）· @node 标识
  // v3 #64 · @node 在线 muted gray / 离线 dim red（区分 stale 节点）
  // #68 (c) · 任务 completed 时 member 副文 fallback 用 "已歇" 替代 "待命"（避免误导用户以为 worker 还在线等接活）
  const memberTail = (m: TaskGroupCard["members"][number]): string => {
    if (m.last_activity) return m.last_activity;
    const tool = toolCallText(m.last_tool_call);
    if (tool) return tool;
    if (props.task.status !== "running") return "已歇";
    return memberStatusFallback(m);
  };
  return (
    <div class={styles.banner} data-testid="task-banner">
      <div class={styles.bannerLine}>
        <span class={`${styles.statusDot} ${statusDotClass()}`} aria-hidden="true" />
        <span class={styles.statusName}>{statusLabel()}</span>
        <span class={styles.bannerSep} aria-hidden="true">·</span>
        <span>{formatDuration(props.task.duration_ms)}</span>
        <span class={styles.bannerSep} aria-hidden="true">·</span>
        <span>{props.task.members.length} 门客</span>
      </div>
      <Show when={props.task.members.length > 0}>
        <div class={styles.bannerMembers} data-testid="task-banner-members">
          <For each={props.task.members}>
            {(m, i) => {
              const tail = truncate(memberTail(m), 12);
              // 仅在 onlineNodeIds 已 ready 且 node_id 不在内时算 offline；未知态默认在线
              const offline = (): boolean =>
                !!m.node_id
                && props.onlineNodeIds !== null
                && !props.onlineNodeIds.has(m.node_id);
              return (
                <>
                  <Show when={i() > 0}>
                    <span aria-hidden="true"> ▎ </span>
                  </Show>
                  <span>
                    {m.role_display} · {tail}
                    <Show when={m.node_id}>
                      {(nid) => (
                        <>
                          {" · "}
                          <span
                            class={offline() ? styles.bannerNodeOffline : ""}
                            data-testid={
                              offline()
                                ? `banner-node-offline-${m.agent_id}`
                                : `banner-node-${m.agent_id}`
                            }
                          >
                            @{nid()}
                          </span>
                        </>
                      )}
                    </Show>
                  </span>
                </>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
};

function toolCallText(call: TaskGroupCard["members"][number]["last_tool_call"]): string | null {
  if (!call) return null;
  if (typeof call === "string") return call;
  return call.args_summary ? `${call.tool} ${call.args_summary}` : call.tool;
}

interface ThreadProps {
  messages: Accessor<Message[]>;
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
    <section ref={scrollEl} class={styles.thread} data-testid="task-thread">
      <Show
        when={props.messages().length > 0}
        fallback={
          <div class={styles.empty} data-testid="task-thread-empty">
            <p class={styles.emptyTitle}>等待第一条消息</p>
            <p class={styles.emptyHint}>{props.online() ? "玄女或门客活动会在此 inline 显示" : "重连中…"}</p>
          </div>
        }
      >
        <For each={props.messages()}>
          {(msg) => {
            if (msg.kind === "user") {
              return (
                <div class={styles.rowRight}>
                  <UserBubble msg={msg} />
                </div>
              );
            }
            if (msg.kind === "xuannv") {
              return (
                <div class={styles.rowLeft}>
                  <XuannvBubble msg={msg} />
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
            if (msg.kind === "thinking") return <ThinkingRow msg={msg} />;
            if (msg.kind === "marker") return <StatusMarkerRow msg={msg} />;
            return null;
          }}
        </For>
      </Show>
    </section>
  );
};

function pickTask(ov: TasksOverview | undefined, taskId: string): TaskGroupCard | null {
  if (!ov) return null;
  return ov.running.find((t) => t.id === taskId) ?? ov.completed.find((t) => t.id === taskId) ?? null;
}

function memberStatusFallback(m: { status?: string }): string {
  if (m.status === "idle") return "待命";
  if (m.status === "thinking") return "思考中";
  return "运行中";
}

function truncate(s: string, n: number): string {
  if (s.length <= n) return s;
  return s.slice(0, n - 1) + "…";
}
