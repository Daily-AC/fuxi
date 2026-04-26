import {
  Show,
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
import type { Upload } from "~/types/api";
import { useApi } from "~/components/ApiProvider";
import { Composer } from "~/components/Composer";
import { Conversation } from "~/views/Conversation";
import styles from "./XuannvPage.module.css";

// Page 2 · 玄女主对话（base camp）
//
// 设计 spec: docs/superpowers/specs/2026-04-26-im-task-tree-redesign-design.md §"页 2·玄女主对话"
//
// composer 走 intervene(xuannv, ...)（默认 active=Xuannv）。
// 顶栏 sticky badge "✓ 抄送 N 门客"（#N4）：
//   - 数据：N = tasksOverview.running.length（已 filter user-turn by β #25）
//   - 仅当 N > 0 时显示
//   - tap badge → setCurrentPage(2) swipe 到任务树
//   - UI 兑现公理 2「玄女永远有知情权」

const RETRY_DELAY_MS = 1500;
const HISTORY_LIMIT = 50;
const CONV_ID = "xuannv";

export const XuannvPage: Component = () => {
  const { client, setCurrentPage } = useApi();

  const [messages, setMessages] = createSignal<Message[]>([]);
  const [online, setOnline] = createSignal(false);

  // #N4 sticky badge · 抄送数 = running tasks 数
  // 用 createResource 拉一次（任务变更走 WS 后续阶段，v1 mount 一次足够）
  const [tasksOverview] = createResource(() => client.fetchTasksOverview());
  const ccCount = (): number => tasksOverview()?.running.length ?? 0;

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

  const attemptIntervene = async (
    text: string,
    attachmentIds: string[],
    msgId: string,
  ): Promise<void> => {
    const send = (): Promise<unknown> =>
      client.intervene({ text, task_id: null, attachments: attachmentIds });
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

  const handleSubmit = async (text: string, attachments: Upload[]): Promise<void> => {
    const m = makeUserMessage(text, attachments.length > 0 ? attachments : undefined);
    setMessages((prev) => [...prev, m]);
    void attemptIntervene(
      text,
      attachments.map((u) => u.id),
      m.id,
    );
  };

  return (
    <div class={styles.page} data-testid="page-xuannv">
      <header class={styles.header}>
        <div class={styles.title}>玄女</div>
        <div class={styles.statusRow}>
          <span class={styles.dot} classList={{ [styles.dotOn ?? ""]: online() }} aria-hidden="true" />
          <span class={styles.status}>{online() ? "在线" : "重连中"}</span>
        </div>
        <Show when={ccCount() > 0}>
          <button
            type="button"
            class={styles.ccBadge}
            onClick={() => setCurrentPage(2)}
            data-testid="cc-badge"
            aria-label={`查看 ${ccCount()} 个进行中任务`}
          >
            <span class={styles.ccCheck} aria-hidden="true">✓</span>
            <span class={styles.ccText}>抄送 {ccCount()} 门客</span>
          </button>
        </Show>
      </header>
      <Conversation messages={messages} />
      <Composer onSubmit={handleSubmit} />
    </div>
  );
};
