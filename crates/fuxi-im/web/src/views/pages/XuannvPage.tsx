import {
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  on,
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
import type { Upload } from "~/types/api";
import type { ServerEvent } from "~/types/events";
import { useApi } from "~/components/ApiProvider";
import { MentionComposer } from "~/components/MentionComposer";
import { Mascot } from "~/components/Mascot/Mascot";
import { useMascot } from "~/components/Mascot/MascotController";
import { Conversation } from "~/views/Conversation";
import {
  candidatesFromMembers,
  candidatesFromNodes,
  candidatesFromProjects,
  sortCandidates,
  type MentionCandidate,
  type SerializedIntervene,
} from "~/lib/mentions";
import { pushToast } from "~/lib/toast";
import { realVoiceDeps } from "~/voice/realVoiceDeps";
import { VoiceController, type VoiceState } from "~/voice/voiceController";
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
/** 语音模式持久开关——"1" 时回到玄女 tab 自动恢复常驻唤醒。 */
const VOICE_MODE_KEY = "fuxi.voice-mode";
/** 安卓壳内（capacitor.config appendUserAgent 标记）禁用 web 语音模式：
 *  原生 VoiceLoopService 全场景接管唤醒/TTS，讯飞引擎进程级单 session，
 *  web 端再开一条必撞 18310 互相抢。PTT 不受影响（不占 wake session）。 */
const IN_SHELL =
  typeof navigator !== "undefined" && navigator.userAgent.includes("FuxiShell");

export const XuannvPage: Component = () => {
  const { client, currentTopicId, setSidebarOpen, isSwitchingTopic } = useApi();
  const { mascotState, dispatch: dispatchMascot } = useMascot();

  const [messages, setMessages] = createSignal<Message[]>([]);
  const [online, setOnline] = createSignal(false);

  // 流式信号 = 当前消息流里有任一玄女/思考气泡处于 streaming 状态。
  // applyEvent reducer 已经在 thinking_started / agent_text_delta 时把对应 bubble
  // 标 streaming:true，在 agent_responded / thinking_done 时翻 false——所以这里
  // 派生一个布尔即可拿到可靠的"玄女正在回话"边界，无需另接 WS 事件类型。
  const streaming = createMemo<boolean>(() =>
    messages().some(
      (m) => (m.kind === "xuannv" || m.kind === "thinking") && m.streaming,
    ),
  );

  // 流式起止驱动头像吉祥物：开始 → talk（说话帧），结束 → idle（回归眨眼）。
  // on(...defer) 避免初次 mount（无流式）误发 stream-end。
  createEffect(
    on(
      () => streaming(),
      (isStreaming) => {
        dispatchMascot({ type: isStreaming ? "stream-start" : "stream-end" });
      },
      { defer: true },
    ),
  );
  // Phase 1 · 当前 topic 的 title 仅用于 header 展示。Sidebar 30s 轮 fetchTopics
  // 是真相源；这里走自己的轻 fetch 拿同一份。topic 切换后 setCurrentTopicId 推
  // 全局态，下面 createEffect 顺手 refetch 拿新 title（用户新建的 topic 也走这条
  // 路径补 title）。两份 fetch 双 GET 一次 30s 一次切换，可接受；scope 内不抽
  // 全局 store。
  const [topicsResp, { refetch: refetchTopicsForTitle }] = createResource(() =>
    client.fetchTopics(false),
  );
  const currentTitle = createMemo<string>(() => {
    const id = currentTopicId();
    const list = topicsResp()?.topics ?? [];
    if (!id) return "";
    const t = list.find((x) => x.id === id);
    return t?.title ?? "";
  });

  // alive workers · 从 running tasks members 去重，不含玄女（role="xuannv"）。
  const [tasksOverview] = createResource(() => client.fetchTasksOverview());
  // v3 #60 dist · 拉 /api/nodes online 节点作 @ 候选追加段
  const [nodesData] = createResource(() => client.fetchNodes());
  // v2 跨节点 · 拉 /api/projects 已注册项目作 @ 候选追加段
  const [projectsData] = createResource(() => client.fetchProjects());
  const candidates = createMemo<MentionCandidate[]>(() => {
    const ov = tasksOverview();
    const workers: MentionCandidate[] = [];
    if (ov) {
      const all = ov.running.flatMap((t) => candidatesFromMembers(t.members));
      const seen = new Set<string>();
      for (const c of all) {
        if (c.role === "xuannv") continue;
        if (seen.has(c.agent_id)) continue;
        seen.add(c.agent_id);
        workers.push(c);
      }
    }
    const sorted = sortCandidates(workers);
    const nodes = nodesData() ? candidatesFromNodes(nodesData()!.nodes) : [];
    const projects = projectsData()
      ? candidatesFromProjects(projectsData()!.projects)
      : [];
    return [...sorted, ...nodes, ...projects];
  });

  let controller: ReconnectController | null = null;

  // ── PWA 语音 · 喊「玄女」唤醒 + 听写 + 回复 TTS（贾维斯同款闭环） ──
  // wake_token null（home 未部署唤醒服务）时隐藏开关，PTT 仍可用。
  const [voiceState, setVoiceState] = createSignal<VoiceState>("off");
  const [voiceAvailable, setVoiceAvailable] = createSignal(false);

  const vc = new VoiceController({
    ...realVoiceDeps(client),
    // 听写文本走 handleSubmit：optimistic 气泡 + 503 重试跟手打同一条路。
    // `[语音] ` 前缀 = jarvis 约定（公理 #8），玄女见此用 say 回口语短句
    // （xuannv_voice_line 带 emotion），长 markdown 不会被硬念出来。
    intervene: (text) =>
      handleSubmit({
        text: `[语音] ${text}`,
        mentions: [],
        multi: false,
        multi_node: false,
        multi_project: false,
      }),
  });
  vc.onState(setVoiceState);
  vc.onError((msg) => pushToast(msg, "error"));

  const toggleVoice = (): void => {
    if (vc.enabled) {
      localStorage.setItem(VOICE_MODE_KEY, "0");
      void vc.disable();
      return;
    }
    vc.enable()
      .then(() => localStorage.setItem(VOICE_MODE_KEY, "1"))
      .catch((e) => {
        pushToast(
          `语音模式开启失败：${e instanceof Error ? e.message : String(e)}`,
          "error",
        );
      });
  };

  onMount(() => {
    if (IN_SHELL) {
      // 壳内原生 VoiceLoopService 接管，web 语音模式不开（开关也不渲染）
      setVoiceAvailable(false);
      return;
    }
    void client
      .voiceTokens()
      .then((t) => {
        setVoiceAvailable(t.wake_token !== null);
        // 上次开着 → 回到玄女 tab 自动恢复（mic 权限已授过则无感重连）
        if (t.wake_token !== null && localStorage.getItem(VOICE_MODE_KEY) === "1") {
          vc.enable().catch((e) => {
            pushToast(
              `语音模式恢复失败：${e instanceof Error ? e.message : String(e)}`,
              "warn",
            );
          });
        }
      })
      .catch(() => setVoiceAvailable(false));
  });

  onCleanup(() => void vc.disable());

  const handleEvent = (ev: ServerEvent): void => {
    setMessages((prev) => applyEvent(prev, ev));
    // Task 32 · 任务完成 → 玄女开心。task_completed 是单一可靠信号（后端
    // EventKind::TaskCompleted），不轮询、不靠 list diff。每条 task_completed
    // 派一次 happy（瞬时态 1.8s 自归位），多任务连完会重置计时不抖。
    if (ev.kind.type === "task_completed") {
      dispatchMascot({ type: "task-done" });
    }
    // 玄女 say 口语短句 → 语音模式下念出来（emotion 透传 sovits 选 ref）
    if (ev.kind.type === "xuannv_voice_line" && typeof ev.kind.text === "string") {
      const emo = ev.kind.emotion;
      vc.onXuannvReply(
        ev.kind.text,
        typeof emo === "string" && emo.length > 0 ? emo : undefined,
      );
    }
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
        // 切回前台时补历史——WS 在 background 被冻结期间错过的消息靠 fetchHistory
        // 兜底；mergeMessages 按 id 去重，重复无副作用。
        onVisible: () => {
          void loadHistory();
        },
      },
    );
  });

  onCleanup(() => {
    controller?.dispose();
    controller = null;
  });

  // Phase 1 · topic 切换时主对话区刷新 —— 后端走 shutdown_xuannv_for_handoff 起新 cc，
  // 老 topic 的历史不再属于新会话语境。清 messages + 重拉 /api/conv/messages（后端
  // conv_store 已按 current_topic_id 过滤）。`defer: true` 避免初次 mount 重复触发 loadHistory。
  // 同时 refetch topicsResp 让 header 副标题拿到新建 topic 的 title。
  createEffect(
    on(
      () => currentTopicId(),
      (id) => {
        if (!id) return;
        setMessages([]);
        void loadHistory();
        void refetchTopicsForTitle();
      },
      { defer: true },
    ),
  );

  const attemptIntervene = async (req: SerializedIntervene, msgId: string): Promise<void> => {
    const send = (): Promise<unknown> =>
      client.intervene({
        text: req.text,
        task_id: null,
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

  const attemptDispatch = async (
    req: SerializedIntervene,
    project: string,
    msgId: string,
  ): Promise<void> => {
    // 派往项目的 task：title 截 body 第一行前 60 字符，description 用全文。
    // 后端 /api/dispatch handler 会按 project.host_nodes auto-pin 到最闲节点 +
    // dist enqueue，worker 端 spawn cc 进对应 sandbox。本流程**不**经 intervene，
    // 玄女不会逐字看到这条 prompt，但会通过 EventBus 收到 TaskCreated/TaskDispatched。
    const firstLine = req.text.split("\n")[0]?.trim() ?? "";
    const title = firstLine.length > 0 ? firstLine.slice(0, 60) : `派给 ${project}`;
    try {
      await client.dispatch({ title, description: req.text, project });
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: null }));
      pushToast(`已派给项目 ${project}`, "info");
    } catch (err) {
      const msg =
        err instanceof ApiError
          ? `派活失败 (${err.status})`
          : err instanceof Error
            ? err.message
            : "派活失败";
      setMessages((prev) => markUserMessage(prev, msgId, { pending: false, error: msg }));
    }
  };

  const handleSubmit = async (req: SerializedIntervene): Promise<void> => {
    // optimistic user bubble · 用 req.text（chip 占位的零宽字符不影响显示）
    // 阶段 3：附件 ids 在 composer 里已上传完成，optimistic 直接挂 placeholder Upload[]
    // 让用户立即看到自己发的图缩略；服务端 ws 事件二次到达会被 mergeMessages 去重。
    const ups: Upload[] | undefined = req.attachments && req.attachments.length > 0
      ? req.attachments.map((id) => ({
          id,
          name: id,
          mime: "application/octet-stream",
          bytes: 0,
          sha256: "",
        }))
      : undefined;
    const m = makeUserMessage(req.text, ups, {
      mentions: req.mentions,
      pinned_node: req.pinned_node,
    });
    setMessages((prev) => [...prev, m]);

    // v2 跨节点 · @<slug> 走 dispatch 路径（创新 task），不污染玄女 thread context
    if (req.project) {
      await attemptDispatch(req, req.project, m.id);
      return;
    }
    await attemptIntervene(req, m.id);
  };

  return (
    <div class={styles.page} data-testid="page-xuannv">
      <header class={styles.header}>
        {/* Phase 1 · 移动端汉堡按钮：唤出 TopicSidebar 抽屉。桌面隐藏（CSS @media）。 */}
        <button
          type="button"
          class={styles.menuBtn}
          onClick={() => setSidebarOpen(true)}
          aria-label="打开话题列表"
          data-testid="topic-drawer-open"
        >
          <span class={styles.menuIcon} aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
        </button>
        <div class={styles.titleStack}>
          <div class={styles.titleRow}>
            <span
              class={styles.avatar}
              classList={{ [styles.avatarTalk ?? ""]: streaming() }}
              data-testid="xuannv-avatar"
              aria-hidden="true"
            >
              <Mascot state={mascotState().kind} size={36} />
            </span>
            <div class={styles.title}>玄女</div>
          </div>
          {/* Phase 1 · 当前 topic 副标题：让用户一眼知道在哪个 topic 聊 */}
          <Show when={currentTitle()}>
            <div class={styles.topicLabel} data-testid="xuannv-topic-label">
              ✻ {currentTitle()}
            </div>
          </Show>
          <div class={styles.statusRow}>
            <span class={styles.dot} classList={{ [styles.dotOn ?? ""]: online() }} aria-hidden="true" />
            <span class={styles.status}>{online() ? "在线" : "重连中"}</span>
          </div>
        </div>
        {/* 右侧占位 · 跟 menuBtn 镜像保持标题居中 */}
        <span class={styles.menuRightSpacer} aria-hidden="true" />
        {/* 语音模式开关 · 绝对定位贴右不挤标题。home 没部署 wake server 时不显。 */}
        <Show when={voiceAvailable()}>
          <button
            type="button"
            class={styles.voiceBtn}
            classList={{
              [styles.voiceBtnOn ?? ""]: voiceState() !== "off",
              [styles.voiceBtnDictating ?? ""]: voiceState() === "dictating",
              [styles.voiceBtnSpeaking ?? ""]: voiceState() === "speaking",
            }}
            onClick={toggleVoice}
            aria-label={voiceState() === "off" ? "开启语音模式" : "关闭语音模式"}
            title={
              voiceState() === "off"
                ? "语音模式：喊「玄女」唤醒"
                : voiceState() === "dictating"
                  ? "听写中…"
                  : voiceState() === "speaking"
                    ? "玄女说话中…"
                    : "聆听中（点击关闭）"
            }
            data-testid="voice-toggle"
          >
            <svg
              viewBox="0 0 24 24"
              width="20"
              height="20"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              aria-hidden="true"
            >
              <path d="M3 10v4" />
              <path d="M7.5 7v10" />
              <path d="M12 4v16" />
              <path d="M16.5 7v10" />
              <path d="M21 10v4" />
            </svg>
          </button>
        </Show>
      </header>
      <Conversation messages={messages} />
      {/* bug B · 切 topic 5-15s 全程显 overlay，让用户知道在切（不是卡住） */}
      <Show when={isSwitchingTopic()}>
        <div
          class={styles.switchOverlay}
          data-testid="topic-switching-overlay"
          aria-live="polite"
        >
          <div class={styles.switchCard}>
            <span class={styles.switchSpinner} aria-hidden="true" />
            <span class={styles.switchLabel}>切换话题中…</span>
            <span class={styles.switchHint}>玄女正在重新接班</span>
          </div>
        </div>
      </Show>
      <MentionComposer
        candidates={candidates()}
        placeholder="对玄女说... (@ 角色或节点)"
        onSubmit={handleSubmit}
        ptt={{ start: () => vc.pttStart(), stop: () => vc.pttStop() }}
      />
    </div>
  );
};
