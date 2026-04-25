import {
  For,
  type Component,
  createSignal,
  onCleanup,
  onMount,
  createEffect,
} from "solid-js";
import { useParams } from "@solidjs/router";
import { useApi } from "~/components/ApiProvider";
import { EventLine } from "~/components/EventLine";
import { cacheEvents, loadCachedEvents } from "~/lib/idb";
import type { EventKind } from "~/types/events";
import styles from "./TaskView.module.css";

// 单 task chat：先 GET /api/tasks/:id/events 取历史，再 WS /api/tasks/:id/stream 接实时增量。
export const TaskView: Component = () => {
  const params = useParams<{ id: string }>();
  const { client } = useApi();
  const [events, setEvents] = createSignal<EventKind[]>([]);
  const [connected, setConnected] = createSignal(false);
  const [streamingByAgent, setStreamingByAgent] = createSignal<Record<string, true>>({});
  let ws: WebSocket | null = null;
  let scrollEnd: HTMLDivElement | undefined;

  const taskId = (): string => params.id;

  const append = (ev: EventKind): void => {
    setEvents([...events(), ev]);
    const k = (ev as { type: string }).type;
    const agent = (ev as { agent?: string }).agent;
    if (agent && k === "agent_text_delta") {
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
  };

  const openSocket = (): void => {
    ws = client.openTaskSocket(taskId());
    ws.addEventListener("open", () => setConnected(true));
    ws.addEventListener("close", () => {
      setConnected(false);
      setTimeout(() => {
        if (!ws || ws.readyState === WebSocket.CLOSED) openSocket();
      }, 1500);
    });
    ws.addEventListener("error", () => setConnected(false));
    ws.addEventListener("message", (e) => {
      try {
        append(JSON.parse(e.data) as EventKind);
      } catch (err) {
        console.warn("task event parse failed", err);
      }
    });
  };

  onMount(async () => {
    const offline = await loadCachedEvents(taskId());
    if (offline.length > 0) setEvents(offline);
    try {
      const hist = await client.fetchTaskEvents(taskId());
      setEvents(hist.events);
      void cacheEvents(hist.events);
    } catch (err) {
      console.warn("history fetch failed", err);
    }
    openSocket();
  });

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
    <section class={styles.view} data-testid="task-view">
      <div class={styles.head}>
        <div class={styles.taskId}>
          任务 <span class="agent-id">#{taskId().slice(0, 8)}</span>
        </div>
        <div class={styles.statusLine}>
          <span class={styles.dot} classList={{ [styles.dotOn ?? ""]: connected() }} />
          <span>{connected() ? "实时" : "离线"}</span>
        </div>
      </div>
      <div class={styles.stream}>
        <For each={events()} fallback={<EmptyTask />}>
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

const EmptyTask: Component = () => (
  <div class={styles.empty}>
    <p class={styles.emptyTitle}>等待事件</p>
    <p class={styles.emptyHint}>顶部输入条可以插话；任务进展会逐条出现。</p>
  </div>
);
