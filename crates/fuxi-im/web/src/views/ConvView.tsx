import {
  For,
  type Component,
  createSignal,
  onCleanup,
  onMount,
  createEffect,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { EventLine } from "~/components/EventLine";
import { cacheEvents, loadCachedEvents } from "~/lib/idb";
import type { EventKind } from "~/types/events";
import styles from "./ConvView.module.css";

// 跟玄女顶层对话流：WS /api/conv，全局事件（不带 task_id 的或 task_id 是 conv 顶层 task 的）。
export const ConvView: Component = () => {
  const { client } = useApi();
  const [events, setEvents] = createSignal<EventKind[]>([]);
  const [connected, setConnected] = createSignal(false);
  const [streamingByAgent, setStreamingByAgent] = createSignal<Record<string, true>>({});
  let ws: WebSocket | null = null;
  let scrollEnd: HTMLDivElement | undefined;

  onMount(async () => {
    const offline = await loadCachedEvents();
    if (offline.length > 0) setEvents(offline);
    open();
  });

  const open = (): void => {
    ws = client.openConvSocket();
    ws.addEventListener("open", () => setConnected(true));
    ws.addEventListener("close", () => {
      setConnected(false);
      setTimeout(() => {
        if (!ws || ws.readyState === WebSocket.CLOSED) open();
      }, 1500);
    });
    ws.addEventListener("error", () => setConnected(false));
    ws.addEventListener("message", (e) => {
      try {
        const ev = JSON.parse(e.data) as EventKind;
        setEvents([...events(), ev]);
        const k = (ev as { type: string }).type;
        const agent = (ev as { agent?: string }).agent;
        if (agent && (k === "agent_text_delta" || k === "agent_busy")) {
          setStreamingByAgent({ ...streamingByAgent(), [agent]: true });
        }
        if (
          agent &&
          (k === "agent_responded" || k === "agent_idle" || k === "result_success")
        ) {
          const next = { ...streamingByAgent() };
          delete next[agent];
          setStreamingByAgent(next);
        }
        void cacheEvents([ev]);
      } catch (err) {
        console.warn("conv event parse failed", err);
      }
    });
  };

  onCleanup(() => {
    ws?.close();
    ws = null;
  });

  createEffect(() => {
    void events();
    queueMicrotask(() => {
      scrollEnd?.scrollIntoView({ block: "end", behavior: "smooth" });
    });
  });

  return (
    <section class={styles.view} data-testid="conv-view">
      <div class={styles.statusLine}>
        <span class={styles.dot} classList={{ [styles.dotOn ?? ""]: connected() }} />
        <span>{connected() ? "在线" : "重连中"}</span>
      </div>
      <div class={styles.stream}>
        <For each={events()} fallback={<EmptyConv />}>
          {(ev) => (
            <EventLine
              ev={ev}
              streaming={Boolean(
                (ev as { agent?: string }).agent &&
                  streamingByAgent()[(ev as { agent: string }).agent],
              )}
            />
          )}
        </For>
        <div ref={scrollEnd} />
      </div>
    </section>
  );
};

const EmptyConv: Component = () => (
  <div class={styles.empty}>
    <p class={styles.emptyTitle}>开个头吧</p>
    <p class={styles.emptyHint}>顶部输入条直接说，玄女在听。</p>
  </div>
);
