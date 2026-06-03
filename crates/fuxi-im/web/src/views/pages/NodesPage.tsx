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
import { colorForTaskRole } from "~/lib/format-task";
import {
  startReconnectingSocket,
  type ReconnectController,
} from "~/lib/reconnect";
import type { NodeView, NodeWorker } from "~/types/api";
import styles from "./NodesPage.module.css";

// 节点 tab · v3 #58 · spec 2026-04-27-im-dist-接通-design.md (gap d)
//
// v2 ε 早期版本走 aggregateHomeNode 假 topology（拿 tasksOverview 包一层），用户实测撞穿
// "home 离线"问题（其实只是当前没 running task）。本版切真 /api/nodes 端点（β #55 落地中），
// dist controller 维护真 node 注册 + heartbeat + workers 实例 map。
//
// 视觉：每节点一卡（#16110E 圆角 12px）；header = 节点名 + online dot + tags chip + inflight/max；
// body = workers 列表（按 status busy>idle）每行 role+task title。离线整卡 muted。
// "添加节点" 按钮弹 modal 显 install script 复制命令。

const INSTALL_CMD = "bash <(curl -s https://im.qmledmq.cn:8443/setup-local-worker.sh)";

export const NodesPage: Component = () => {
  return (
    <div class={styles.page} data-testid="page-nodes">
      <header class={styles.header}>
        <h1 class={styles.title}>节点</h1>
        <AddNodeButton />
      </header>
      <div class={styles.body}>
        <RenderNodes />
      </div>
    </div>
  );
};

const AddNodeButton: Component = () => {
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <button
        type="button"
        class={styles.addBtn}
        onClick={() => setOpen(true)}
        data-testid="nodes-add-btn"
        aria-label="添加节点"
      >
        + 添加节点
      </button>
      <Show when={open()}>
        <AddNodeModal onClose={() => setOpen(false)} />
      </Show>
    </>
  );
};

const AddNodeModal: Component<{ onClose: () => void }> = (props) => {
  const onCopy = (): void => {
    void navigator.clipboard?.writeText(INSTALL_CMD);
  };
  return (
    <div
      class={styles.modalScrim}
      data-testid="nodes-add-modal"
      onClick={(e) => {
        if (e.target === e.currentTarget) props.onClose();
      }}
    >
      <div class={styles.modalCard} role="dialog" aria-label="添加节点">
        <header class={styles.modalHead}>
          <h2 class={styles.modalTitle}>添加节点</h2>
          <button
            type="button"
            class={styles.modalClose}
            onClick={() => props.onClose()}
            aria-label="关闭"
            data-testid="nodes-add-close"
          >
            ×
          </button>
        </header>
        <p class={styles.modalHint}>
          在新设备上跑这条命令，输入主密码即可加入：
        </p>
        <pre class={styles.modalCode} data-testid="nodes-install-cmd">
          {INSTALL_CMD}
        </pre>
        <button
          type="button"
          class={styles.modalCopy}
          onClick={onCopy}
          data-testid="nodes-add-copy"
        >
          复制命令
        </button>
        <p class={styles.modalNote}>
          支持 macOS（launchd）+ Linux（systemd user）。安装后该节点自动出现在列表，离线
          ≥ 30s 显灰。
        </p>
      </div>
    </div>
  );
};

const RenderNodes: Component = () => {
  const { client } = useApi();
  // 用 createResource 的 refetch 让 WS 推送到来时重拉 /api/nodes——后端
  // /api/nodes/stream 只发"节点拓扑变化"信号（WorkerRegistered /
  // WorkerHeartbeatStateChanged / WorkerStaleSwept），不传完整 NodeView，
  // 避免前后端各算一遍 home shelf + dist topology 算法走偏。
  const [data, { refetch }] = createResource(() => client.fetchNodes());
  let controller: ReconnectController | null = null;

  onMount(() => {
    controller = startReconnectingSocket(
      () => client.openNodesStreamSocket(),
      {
        // 收到任何节点级事件就 refetch——后端 ws_common::run_ws_loop 已对
        // 三类 EventKind 做白名单过滤；前端拿到即视作"该刷"。
        onMessage: () => {
          void refetch();
        },
        // 切回前台 = 拓扑可能在 background 期间变了，refetch 兜底
        onVisible: () => {
          void refetch();
        },
      },
    );
  });
  onCleanup(() => {
    controller?.dispose();
    controller = null;
  });

  const nodes = createMemo<NodeView[]>(() => {
    const list = data()?.nodes ?? [];
    // online 在前，online 内部按 inflight 降序（最忙优先）
    return list.slice().sort((a, b) => {
      if (a.online !== b.online) return a.online ? -1 : 1;
      return b.inflight_jobs - a.inflight_jobs;
    });
  });

  return (
    <Show
      when={data() || data.error}
      fallback={<p class={styles.muted} data-testid="nodes-loading">加载中…</p>}
    >
      <Show when={data.error}>
        <p class={styles.errMsg} role="alert">
          加载失败：{String(data.error)}
        </p>
      </Show>
      <Show
        when={nodes().length > 0}
        fallback={
          <div class={styles.empty} data-testid="nodes-empty">
            <p class={styles.emptyTitle}>暂无节点</p>
            <p class={styles.emptyHint}>点上方"+ 添加节点"接入第一个 worker</p>
          </div>
        }
      >
        <div class={styles.root}>
          <For each={nodes()}>{(n) => <NodeCard node={n} />}</For>
        </div>
      </Show>
    </Show>
  );
};

const NodeCard: Component<{ node: NodeView }> = (props) => {
  const sortedWorkers = createMemo<NodeWorker[]>(() => {
    return props.node.workers.slice().sort((a, b) => rank(b.status) - rank(a.status));
  });
  return (
    <article
      class={styles.nodeCard}
      classList={{ [styles.nodeCardOffline ?? ""]: !props.node.online }}
      data-testid={`node-${props.node.node_id}`}
      data-online={props.node.online ? "true" : "false"}
    >
      <header class={styles.nodeHead}>
        <span
          class={styles.nodeDot}
          classList={{ [styles.nodeDotOn ?? ""]: props.node.online }}
          aria-hidden="true"
        />
        <span class={styles.nodeName}>{props.node.node_id}</span>
        <span class={styles.nodeMeta}>
          {props.node.inflight_jobs}/{props.node.max_concurrency}
        </span>
      </header>
      <Show when={props.node.tags.length > 0}>
        <div class={styles.tagRow}>
          <For each={props.node.tags}>
            {(t) => <span class={styles.tag}>{t}</span>}
          </For>
        </div>
      </Show>
      <Show when={props.node.online && sortedWorkers().length > 0}>
        <ul class={styles.workers}>
          <For each={sortedWorkers()}>{(w) => <WorkerRow worker={w} />}</For>
        </ul>
      </Show>
      <Show when={props.node.online && sortedWorkers().length === 0}>
        <p class={styles.workerEmpty}>当前无 worker 实例</p>
      </Show>
    </article>
  );
};

const WorkerRow: Component<{ worker: NodeWorker }> = (props) => {
  const { navTo } = useApi();
  const onTap = (): void => {
    if (!props.worker.current_task_id) return;
    // navTo 原子切到任务 tab(2) + push task 路由，避免 setActiveTab 清栈与 navPush 两步 race。
    navTo({
      kind: "task",
      task_id: props.worker.current_task_id,
      title: props.worker.current_task_title ?? undefined,
    });
  };
  const tappable = (): boolean => Boolean(props.worker.current_task_id);
  // P2-A · "已完成等清理" 视觉折叠：worker idle 且无 current_task = 跑完一轮等 IdleGc。
  // 跟"真等派活的 idle"无后端区分（status 都是 idle），但 UX 上要让用户一眼看出
  // "这门客刚跑完不必再派"。CSS .workerRowSettled 把行压扁 + 灰显 + 加小标签。
  // 召回机制（fuxi spawn --recall-role）在玄女那边知情，用户看到 "等清理" 文案
  // 不会误以为该门客还接活——要再用走召回起新进程。
  const isSettled = (): boolean =>
    props.worker.status === "idle" && !props.worker.current_task_id;
  return (
    <li
      class={styles.workerRow}
      classList={{ [styles.workerRowSettled ?? ""]: isSettled() }}
      data-testid={`node-worker-${props.worker.agent_id}`}
      data-status={props.worker.status}
      data-settled={isSettled() ? "true" : "false"}
    >
      <button
        type="button"
        class={styles.workerBtn}
        onClick={onTap}
        disabled={!tappable()}
        aria-label={
          tappable()
            ? `跳转任务 ${props.worker.current_task_title ?? ""}`
            : `${props.worker.role_display} ${statusLabel(props.worker.status)}${
                isSettled() ? "（已完成等清理）" : ""
              }`
        }
      >
        <span
          class={styles.workerDot}
          style={{ background: colorForTaskRole(props.worker.role) }}
          aria-hidden="true"
        />
        <span class={styles.workerRole}>{props.worker.role_display}</span>
        <Show
          when={props.worker.current_task_title}
          fallback={<span class={styles.workerStatusText}>{statusLabel(props.worker.status)}</span>}
        >
          <span class={styles.workerTaskTitle}>{props.worker.current_task_title}</span>
        </Show>
        <Show when={isSettled()}>
          <span class={styles.settledBadge} data-testid={`worker-settled-${props.worker.agent_id}`}>
            等清理
          </span>
        </Show>
      </button>
    </li>
  );
};

function rank(s: NodeWorker["status"]): number {
  if (s === "busy") return 3;
  if (s === "thinking") return 2;
  return 1; // idle
}

function statusLabel(s: NodeWorker["status"]): string {
  if (s === "busy") return "运行中";
  if (s === "thinking") return "思考中";
  return "待命";
}
