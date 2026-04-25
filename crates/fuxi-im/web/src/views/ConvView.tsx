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
import { startReconnectingSocket, type ReconnectController } from "~/lib/reconnect";
import type { EventKind } from "~/types/events";
import styles from "./ConvView.module.css";

// 跟玄女顶层对话流：WS /api/conv，全局事件（不带 task_id 的或 task_id 是 conv 顶层 task 的）。
// Bug 14B：仅在本视图 mount 时建 WS；onCleanup 通过 controller.dispose() 同时
// 关 socket + 取消重连 timer，防止离开页面后 setTimeout 起新连接酿成风暴。
export const ConvView: Component = () => {
  const { client } = useApi();
  const [events, setEvents] = createSignal<EventKind[]>([]);
  const [connected, setConnected] = createSignal(false);
  const [streamingByAgent, setStreamingByAgent] = createSignal<Record<string, true>>({});
  let controller: ReconnectController | null = null;
  let scrollEnd: HTMLDivElement | undefined;

  onMount(async () => {
    const offline = await loadCachedEvents();
    if (offline.length > 0) setEvents(offline);
    controller = startReconnectingSocket(
      () => client.openConvSocket(),
      {
        onOpen: () => setConnected(true),
        onClose: () => setConnected(false),
        onError: () => setConnected(false),
        onMessage: (e) => {
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
        },
      },
    );
  });

  onCleanup(() => {
    controller?.dispose();
    controller = null;
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
