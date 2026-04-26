import {
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  type Component,
} from "solid-js";
import { ApiError } from "~/lib/api";
import { startReconnectingSocket, type ReconnectController } from "~/lib/reconnect";
import {
  applyEvent,
  fromStoredMessage,
  makeUserMessage,
  markUserMessage,
  mergeMessages,
  type Message,
} from "~/messages";
import type { ServerEvent } from "~/types/events";
import { useApi } from "~/components/ApiProvider";
import { MentionComposer } from "~/components/MentionComposer";
import { Conversation } from "~/views/Conversation";
import {
  candidatesFromMembers,
  sortCandidates,
  type MentionCandidate,
  type SerializedIntervene,
} from "~/lib/mentions";
import styles from "./XuannvPage.module.css";

// 玄女 tab · v3 #N5' / #40
//
// 设计 spec: docs/superpowers/specs/2026-04-26-im-tab-bar-task-thread-design.md §"玄女 tab"
//
// 改动 vs v2：
//   - 删 sticky badge "✓ 抄送 N 门客"（任务 tab 自身已是这个角色 redundant）
//   - 加 MentionComposer（@ chip + autocomplete）
//   - 候选 = 全 alive workers（tasksOverview.running 内成员去重，不含玄女）
//   - 默认对玄女说（无 chip 时 backend 走玄女默认）
//   - 有 @ → intervene 带 target=mentioned_agent_id；回话仍显在玄女 thread（不切页）
//
// 公理 2「玄女永远有知情权」 v3 兑现路径：玄女对所有 worker 派活有 dispatch 抄送（后端 A2A 层）。

const RETRY_DELAY_MS = 1500;
const HISTORY_LIMIT = 50;
const CONV_ID = "xuannv";

export const XuannvPage: Component = () => {
  const { client } = useApi();

  const [messages, setMessages] = createSignal<Message[]>([]);
  const [online, setOnline] = createSignal(false);

  // alive workers · 从 running tasks members 去重，不含玄女（role="xuannv"）。
  const [tasksOverview] = createResource(() => client.fetchTasksOverview());
  const candidates = createMemo<MentionCandidate[]>(() => {
    const ov = tasksOverview();
    if (!ov) return [];
    const all = ov.running.flatMap((t) => candidatesFromMembers(t.members));
    // 去重 by agent_id + 过滤掉玄女
    const seen = new Set<string>();
    const out: MentionCandidate[] = [];
    for (const c of all) {
      if (c.role === "xuannv") continue;
      if (seen.has(c.agent_id)) continue;
      seen.add(c.agent_id);
      out.push(c);
    }
    return sortCandidates(out);
  });

  let controller: ReconnectController | null = null;

  const handleEvent = (ev: ServerEvent): void => {
    setMessages((prev) => applyEvent(prev, ev));
  };

  const loadHistory = async (): Promise<void> => {
    try {
      const r = await client.fetchHistory(CONV_ID, HISTORY_LIMIT);
      const seeded = r.messages
        .map(fromStoredMessage)
        .filter((m): m is Message => m !== null);
      if (seeded.length > 0) setMessages((prev) => mergeMessages(prev, seeded));
    } catch (err) {
      console.warn("history load failed", err);
    }
  };

  onMount(() => {
    void loadHistory();
    controller = startReconnectingSocket(
      () => client.openConvSocket(),
      {
        onOpen: () => setOnline(true),
        onClose: () => setOnline(false),
        onError: () => setOnline(false),
        onMessage: (e) => {
          try {
            const ev = JSON.parse(e.data) as ServerEvent;
            handleEvent(ev);
          } catch (err) {
            console.warn("conv event parse failed", err);
          }
        },
      },
    );
  });

  onCleanup(() => {
    controller?.dispose();
    controller = null;
  });

  const attemptIntervene = async (req: SerializedIntervene, msgId: string): Promise<void> => {
    const send = (): Promise<unknown> =>
      client.intervene({
        text: req.text,
        task_id: null,
        target: req.target,
        mentions: req.mentions.length > 0 ? req.mentions : undefined,
      });
    try {
      await send();
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: null }));
      return;
    } catch (err) {
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
              ? "玄女后端不在 / 服务暂忙，稍后再试"
              : err2 instanceof Error
                ? err2.message
                : "发送失败";
          setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: msg }));
          return;
        }
      }
      const fallback =
        err instanceof ApiError && err.status === 401
          ? "未登入或会话过期"
          : err instanceof Error
            ? err.message
            : "发送失败";
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: fallback }));
    }
  };

  const handleSubmit = async (req: SerializedIntervene): Promise<void> => {
    // optimistic user bubble · 用 req.text（chip 占位的零宽字符不影响显示）
    const m = makeUserMessage(req.text);
    setMessages((prev) => [...prev, m]);
    await attemptIntervene(req, m.id);
  };

  return (
    <div class={styles.page} data-testid="page-xuannv">
      <header class={styles.header}>
        <div class={styles.title}>玄女</div>
        <div class={styles.statusRow}>
          <span class={styles.dot} classList={{ [styles.dotOn ?? ""]: online() }} aria-hidden="true" />
          <span class={styles.status}>{online() ? "在线" : "重连中"}</span>
        </div>
      </header>
      <Conversation messages={messages} />
      <MentionComposer
        candidates={candidates()}
        placeholder="对玄女说..."
        onSubmit={handleSubmit}
      />
    </div>
  );
};
