import { Show, createSignal, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { SectionLabel } from "~/components/ui/SectionLabel";
import { ToggleRow } from "~/components/ui/ToggleRow";
import { ListRow } from "~/components/ui/ListRow";
import styles from "./SettingsPage.module.css";

// 「更多 → 设置」· daimeng 奶油糖果 · archetype D 设置/表单。
//
// 分区：
//   1. 推送通知（pushPermission + enablePush 复用）— 值行 + 开启按钮
//   2. 玄女吉祥物 — 两个 ToggleRow（桌宠动效 / 戳一戳俏皮话），persist localStorage
//   3. 关于（PWA build version——静态文案）
//
// 桌宠两开关只本地 persist（不在本任务接进 Mascot 行为），默认 true。

const ANIM_KEY = "fuxi-mascot-anim";
const QUIPS_KEY = "fuxi-mascot-quips";

function readBool(key: string, fallback: boolean): boolean {
  if (typeof localStorage === "undefined") return fallback;
  const v = localStorage.getItem(key);
  if (v == null) return fallback;
  return v === "1" || v === "true";
}

function writeBool(key: string, value: boolean): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(key, value ? "1" : "0");
}

// 推送通知图标（铃铛）
const BellIcon = (): ReturnType<Component> => (
  <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
    <path
      d="M18 8a6 6 0 1 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
    <path
      d="M13.7 21a2 2 0 0 1-3.4 0"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
  </svg>
);

// 关于图标（信息）
const InfoIcon = (): ReturnType<Component> => (
  <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
    <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="1.8" />
    <path
      d="M12 11v5"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
    />
    <circle cx="12" cy="7.6" r="1.1" fill="currentColor" />
  </svg>
);

export const SettingsPage: Component = () => {
  const { pushPermission, enablePush } = useApi();
  const [mascotAnim, setMascotAnim] = createSignal(readBool(ANIM_KEY, true));
  const [mascotQuips, setMascotQuips] = createSignal(readBool(QUIPS_KEY, true));

  const permLabel = (): string => {
    switch (pushPermission()) {
      case "granted":
        return "已开启";
      case "denied":
        return "已拒（系统设置改）";
      case "unsupported":
        return "本设备不支持";
      default:
        return "未开启";
    }
  };
  const canEnable = (): boolean =>
    pushPermission() !== "granted" && pushPermission() !== "unsupported";

  const onAnimChange = (next: boolean): void => {
    setMascotAnim(next);
    writeBool(ANIM_KEY, next);
  };
  const onQuipsChange = (next: boolean): void => {
    setMascotQuips(next);
    writeBool(QUIPS_KEY, next);
  };

  return (
    <div class={`u-mesh ${styles.page}`} data-testid="page-settings">
      {/* ===== 推送通知 ===== */}
      <SectionLabel>通知</SectionLabel>
      <ListRow
        icon={<BellIcon />}
        title="推送通知"
        subtitle="玄女 idle / 任务完成 / 红点会推到本设备"
        right={<span class={styles.value}>{permLabel()}</span>}
      />
      <Show when={canEnable()}>
        <button
          type="button"
          class={styles.btn}
          onClick={() => void enablePush()}
          data-testid="settings-enable-push"
        >
          开启推送
        </button>
      </Show>

      {/* ===== 玄女吉祥物 ===== */}
      <SectionLabel>玄女吉祥物</SectionLabel>
      <ToggleRow
        title="桌宠动效"
        subtitle="呼吸 / 眨眼 / 反应"
        checked={mascotAnim()}
        onChange={onAnimChange}
      />
      <ToggleRow
        title="戳一戳俏皮话"
        checked={mascotQuips()}
        onChange={onQuipsChange}
      />

      {/* ===== 关于 ===== */}
      <SectionLabel>关于</SectionLabel>
      <div data-testid="settings-about">
        <ListRow
          icon={<InfoIcon />}
          title="fuxi PWA"
          subtitle="伏羲个人 AI agent 平台 ·「更多」hub 重构"
        />
      </div>
    </div>
  );
};
