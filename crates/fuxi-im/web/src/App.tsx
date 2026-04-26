import {
  Show,
  createSignal,
  onCleanup,
  onMount,
  type Component,
  type JSX,
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
import { ApiProvider, useApi } from "./components/ApiProvider";
import { LoginView } from "./components/LoginView";
import { Header } from "./components/Header";
import { Composer } from "./components/Composer";
import { Conversation } from "./views/Conversation";
import { TasksSheet } from "./views/sheets/TasksSheet";
import { NodesSheet } from "./views/sheets/NodesSheet";
import styles from "./App.module.css";

// 顶层 shell：未登入 → LoginView；登入 → Header + Conversation + Composer。
// 阶段 3：拉历史 + WS 流入去重 + intervene 带 attachments。
export const App: Component = (): JSX.Element => {
  onMount(() => {
    if ("serviceWorker" in navigator && import.meta.env.PROD) {
      navigator.serviceWorker.register("/sw.js").catch(() => undefined);
    }
  });

  return (
    <ApiProvider>
      <AuthGate />
    </ApiProvider>
  );
};

const AuthGate: Component = () => {
  const { authState, markLoggedIn } = useApi();
  return (
    <>
      <Show when={authState() === "in"}>
        <MainShell />
      </Show>
      <Show when={authState() === "out"}>
        <LoginView onSuccess={() => markLoggedIn()} />
      </Show>
      <Show when={authState() === "unknown"}>
        <div class={styles.probing} data-testid="auth-probing" aria-hidden="true" />
      </Show>
    </>
  );
};

const RETRY_DELAY_MS = 1500;
const HISTORY_LIMIT = 50;
const CONV_ID = "xuannv"; // 主对话固定 id（β #17 采用此 conv_id 作 fixed top-level）

const MainShell: Component = () => {
  const { client, setActiveSheet } = useApi();

  const [messages, setMessages] = createSignal<Message[]>([]);
  const [online, setOnline] = createSignal(false);

  let controller: ReconnectController | null = null;

  const handleEvent = (ev: ServerEvent): void => {
    setMessages((prev) => applyEvent(prev, ev));
  };

  // 历史预加载：mount 后立即拉一次 GET /api/conv/messages。
  // β #17 还在做；fetch 失败时 silent fallback 到空状态，让 WS 推后续。
  const loadHistory = async (): Promise<void> => {
    try {
      const r = await client.fetchHistory(CONV_ID, HISTORY_LIMIT);
      const seeded = r.messages
        .map(fromStoredMessage)
        .filter((m): m is Message => m !== null);
      if (seeded.length > 0) {
        setMessages((prev) => mergeMessages(prev, seeded));
      }
    } catch (err) {
      console.warn("history load failed (β #17 may not be ready)", err);
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

  // 503 退避重试 1 次；其它 ApiError 标记 inline error。
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
    // optimistic：立即插用户 bubble；输入 / chip 清空由 Composer 自己做
    const m = makeUserMessage(text, attachments.length > 0 ? attachments : undefined);
    setMessages((prev) => [...prev, m]);
    void attemptIntervene(
      text,
      attachments.map((u) => u.id),
      m.id,
    );
  };

  return (
    <div class={styles.shell} data-testid="main-shell">
      <Header
        online={online()}
        onOpenTasks={() => setActiveSheet("tasks")}
        onOpenNodes={() => setActiveSheet("nodes")}
      />
      <Conversation messages={messages} />
      <Composer onSubmit={handleSubmit} />
      <TasksSheet />
      <NodesSheet />
    </div>
  );
};
