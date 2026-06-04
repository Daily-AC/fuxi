import {
  createResource,
  createMemo,
  onMount,
  Show,
  type Component,
  type JSX,
} from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { useMascot } from "~/components/Mascot/MascotController";
import { Mascot } from "~/components/Mascot/Mascot";
import { Tile } from "~/components/ui/Tile";
import { Skeleton } from "~/components/ui/Skeleton";
import styles from "./HomePage.module.css";

// 家·客厅主屏 · daimeng 奶油糖果重构 · Task 15
//
// 用户进 app 看到的第一屏：玄女 mascot 居中 hero + 时段问候 + 在跑/通知状态
// + 2x2 快捷入口。质感走 u-mesh + u-noise（暖光网格 + 颗粒），卡片柔影。
// 入口 / mascot 进场 wave、戳一下出 quip——让她「活」。

// 时段问候：按本地小时切早安/午安/晚上好。
function greetingPrefix(hour: number): string {
  if (hour < 5) return "夜深了";
  if (hour < 11) return "早安";
  if (hour < 14) return "午安";
  if (hour < 18) return "下午好";
  return "晚上好";
}

// 快捷入口图标（inline SVG，禁 emoji）。统一 stroke 风格，色由 iconSlot 染。
const ChatIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path
      d="M4 6.5A2.5 2.5 0 0 1 6.5 4h11A2.5 2.5 0 0 1 20 6.5v7A2.5 2.5 0 0 1 17.5 16H10l-4 3.5V16H6.5A2.5 2.5 0 0 1 4 13.5v-7Z"
      stroke-linejoin="round"
    />
  </svg>
);
const TasksIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M9 6h10M9 12h10M9 18h10" stroke-linecap="round" />
    <path d="m4 5.5 1.4 1.4L8 4M4 11.5l1.4 1.4L8 10M4 17.5l1.4 1.4L8 16" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);
const BellIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path
      d="M6 16V10.5a6 6 0 0 1 12 0V16l1.5 2.5h-15L6 16Z"
      stroke-linejoin="round"
    />
    <path d="M10 19.5a2 2 0 0 0 4 0" stroke-linecap="round" />
  </svg>
);
const BoxIcon = (): JSX.Element => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
    <path d="M4 8.5 12 4l8 4.5v7L12 20l-8-4.5v-7Z" stroke-linejoin="round" />
    <path d="m4 8.5 8 4.5 8-4.5M12 13v7" stroke-linejoin="round" />
  </svg>
);

export const HomePage: Component<{ unreadCount?: number }> = (props) => {
  const { client, setActiveTab, setMoreSub, openNotifications } = useApi();
  const { mascotState, dispatch, poke, quip } = useMascot();

  const [overview] = createResource(() => client.fetchTasksOverview());

  // 进场招手——让玄女上来就 wave 一下打招呼。
  onMount(() => dispatch({ type: "greet" }));

  const hour = new Date().getHours();
  const greeting = `${greetingPrefix(hour)}，以琳`;

  const runningCount = createMemo(() => overview()?.running.length ?? 0);
  const unread = (): number => props.unreadCount ?? 0;

  // 气泡文案：戳了有 quip 就显示 quip，否则默认问候。
  const bubbleText = createMemo(() => quip() ?? "今天想做点什么呀～");

  // 交付物入口：进「更多」hub 的 deliverables 子页。先切 tab 4(更多) 再设 sub。
  const goDeliverables = (): void => {
    setActiveTab(4);
    setMoreSub("deliverables");
  };

  return (
    <div class={`u-mesh u-noise ${styles.page}`} data-testid="page-home">
      <div class={styles.inner}>
        <h1 class={`u-title ${styles.greeting}`} data-testid="home-greeting">
          {greeting}
        </h1>

        <div class={styles.hero}>
          <div class={styles.bubble} aria-hidden="true">
            {bubbleText()}
          </div>
          <div class={styles.mascotWrap} data-testid="home-mascot">
            <Mascot state={mascotState().kind} size={180} onPoke={poke} />
          </div>
        </div>

        <div class={`u-card ${styles.status}`} data-testid="home-status">
          <span class={styles.statusDot} aria-hidden="true" />
          <Show
            when={overview.state === "ready"}
            fallback={<Skeleton width="160px" height="18px" radius="9px" />}
          >
            <span class={styles.statusText}>
              门客在跑 <b>{runningCount()}</b> · <b>{unread()}</b> 条新通知
            </span>
          </Show>
        </div>

        <div class={styles.grid}>
          <div data-testid="home-action-chat">
            <Tile
              icon={<ChatIcon />}
              label="找玄女聊"
              desc="跟玄女说说话"
              onClick={() => setActiveTab(0)}
            />
          </div>
          <div data-testid="home-action-tasks">
            <Tile
              icon={<TasksIcon />}
              label="看任务"
              desc="门客都在忙啥"
              onClick={() => setActiveTab(3)}
            />
          </div>
          <div data-testid="home-action-notifications">
            <Tile
              icon={<BellIcon />}
              label="看通知"
              desc="最新动静"
              onClick={() => openNotifications()}
            />
          </div>
          <div data-testid="home-action-deliverables">
            <Tile
              icon={<BoxIcon />}
              label="交付物"
              desc="门客交付收件箱"
              onClick={goDeliverables}
            />
          </div>
        </div>
      </div>
    </div>
  );
};
