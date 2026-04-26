import { Show, type Component, type JSX } from "solid-js";
import { colorForRole } from "~/tokens";
import styles from "./MentionChip.module.css";

// MentionChip · v3 #N2' / #37
// 设计 spec: 2026-04-26-im-tab-bar-task-thread-design.md §C
//
// 用法两路：
//   1. composer 内 chip token（removable=true，提供 onRemove 删 chip）
//   2. 历史消息内还原（removable=false）
//
// 视觉：圆角 + 角色色 dot + role 名 mono + 可选 ✕。
// 颜色：角色色（鲁班琥珀 / 蒲松绿 / 玄女紫 / unknown 灰）。
// chip 不参与可访问 tab order（aria-hidden 不要——有 ✕ 时仍可 focus）。

export interface MentionChipProps {
  agent_id: string;
  role: string;
  role_display: string;
  /** true 时显 ✕ 删除按钮 + 走 onRemove。默认 false。*/
  removable?: boolean;
  onRemove?: () => void;
}

export const MentionChip: Component<MentionChipProps> = (props) => {
  const color = (): string => colorForRole(props.role);
  // chip 背景：role_color 的 alpha 0.15（spec §C 写的 rgba(229,165,71,.15) 鲁班例）
  const style = (): JSX.CSSProperties => ({
    "--chip-color": color(),
    "--chip-bg": `${color()}26`, // 26 hex = ~0.15 alpha
    "--chip-border": color(),
  });

  return (
    <span
      class={styles.chip}
      style={style()}
      data-testid={`mention-chip-${props.agent_id}`}
      data-agent-id={props.agent_id}
      data-role={props.role}
    >
      <span class={styles.dot} aria-hidden="true" />
      <span class={styles.label}>@{props.role_display}</span>
      <Show when={props.removable}>
        <button
          type="button"
          class={styles.remove}
          aria-label={`移除 @${props.role_display}`}
          data-testid={`mention-chip-remove-${props.agent_id}`}
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            props.onRemove?.();
          }}
        >
          ×
        </button>
      </Show>
    </span>
  );
};
