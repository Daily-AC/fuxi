import {
  For,
  Show,
  createMemo,
  createResource,
  type Component,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { BottomSheet } from "~/components/BottomSheet";
import { colorForTaskRole, formatTokens } from "~/lib/format-task";
import type { TaskMember, TasksOverview } from "~/types/api";
import styles from "./NodesSheet.module.css";

// 节点 sheet · v1 mock：从 /api/tasks 返回的 members 反推单 "home" 节点。
// 多节点（erp-laptop / home-old）等 B 路分布式 ship 后再扩。

interface NodeAgentRow {
  agent_id: string;
  role: string;
  role_display: string;
  status: TaskMember["status"];
  tokens: number;
}

interface NodeView {
  id: string;
  name: string;
  online: boolean;
  agents: NodeAgentRow[];
}

/** 把 overview 聚合成单 home 节点。同 agent_id 出现多次（同 agent 在多个 task）→ tokens 累加；
 *  status 取最忙（busy > thinking > idle）。空 overview → 空节点（仍显示 "home offline"）。*/
export function aggregateHomeNode(overview: TasksOverview | undefined): NodeView {
  const buckets = new Map<string, NodeAgentRow>();
  if (overview) {
    for (const t of overview.running) {
      for (const m of t.members) {
        const exist = buckets.get(m.agent_id);
        if (exist) {
          exist.tokens += m.tokens ?? 0;
          if (rank(m.status) > rank(exist.status)) exist.status = m.status;
        } else {
          buckets.set(m.agent_id, {
            agent_id: m.agent_id,
            role: m.role,
            role_display: m.role_display,
            status: m.status,
            tokens: m.tokens ?? 0,
          });
        }
      }
    }
  }
  const agents = Array.from(buckets.values()).sort((a, b) => {
    if (rank(a.status) !== rank(b.status)) return rank(b.status) - rank(a.status);
    return a.role_display.localeCompare(b.role_display, "zh");
  });
  return {
    id: "home",
    name: "home",
    online: agents.length > 0,
    agents,
  };
}

function rank(s: TaskMember["status"]): number {
  if (s === "busy") return 3;
  if (s === "thinking") return 2;
  return 1; // idle
}

function statusLabel(s: TaskMember["status"]): string {
  if (s === "busy") return "运行中";
  if (s === "thinking") return "思考中";
  return "空闲";
}

export const NodesSheet: Component = () => {
  const { activeSheet, setActiveSheet } = useApi();
  const open = (): boolean => activeSheet() === "nodes";

  return (
    <BottomSheet
      open={open()}
      onClose={() => setActiveSheet(null)}
      title="节点"
      testId="nodes-sheet"
    >
      <Show when={open()}>
        <RenderNodes />
      </Show>
    </BottomSheet>
  );
};

const RenderNodes: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchTasksOverview());
  const node = createMemo(() => aggregateHomeNode(data()));

  return (
    <Show
      when={data() || data.error}
      fallback={<p class={styles.muted} data-testid="nodes-loading">加载中…</p>}
    >
      <div class={styles.root}>
        <Show when={data.error}>
          <p class={styles.errMsg} role="alert">
            加载失败：{String(data.error)}
          </p>
        </Show>
        <section class={styles.section} data-testid={node().online ? "nodes-online" : "nodes-offline"}>
          <h3 class={styles.sectionLabel}>{node().online ? "在线" : "离线"}</h3>
          <article class={styles.nodeCard} data-testid={`node-${node().id}`}>
            <header class={styles.nodeHead}>
              <span class={styles.nodeName}>{node().name}</span>
              <span class={styles.nodeStatus}>
                <span
                  class={styles.statusDot}
                  classList={{ [styles.statusDotOn ?? ""]: node().online }}
                  aria-hidden="true"
                />
                <span class={styles.statusText}>{node().online ? "在线" : "离线"}</span>
              </span>
            </header>
            <Show
              when={node().agents.length > 0}
              fallback={<p class={styles.muted}>当前没有活跃 agent</p>}
            >
              <ul class={styles.agents}>
                <For each={node().agents}>
                  {(a) => (
                    <li class={styles.agentRow} data-testid={`node-agent-${a.agent_id}`}>
                      <span
                        class={styles.agentDot}
                        style={{ background: colorForTaskRole(a.role) }}
                        aria-hidden="true"
                      />
                      <span class={styles.agentName}>{a.role_display}</span>
                      <span class={styles.agentStatusText}>{statusLabel(a.status)}</span>
                      <Show when={a.tokens > 0}>
                        <span class={`${styles.agentTokens} mono`}>{formatTokens(a.tokens)}</span>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </article>
        </section>
      </div>
    </Show>
  );
};
