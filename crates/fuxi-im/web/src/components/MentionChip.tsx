import { Show, type Component, type JSX } from "solid-js";
import { colorForRole } from "~/tokens";
import { NODE_CHIP_COLOR, type MentionKind } from "~/lib/mentions";
import styles from "./MentionChip.module.css";

// MentionChip · v3 #N2' / #37 + dist #60
// 设计 spec:
//   - 2026-04-26-im-tab-bar-task-thread-design.md §C（worker chip 现有）
//   - 2026-04-27-im-dist-接通-design.md §"gap e"（node chip · 蓝色 #7AA0E5 区分）
//
// 用法两路：
//   1. composer 内 chip token（removable=true，提供 onRemove 删 chip）
//   2. 历史消息内还原（removable=false）
//
// 视觉：圆角 + 颜色 dot + 名字 mono + 可选 ✕
// 颜色：worker = colorForRole(role)；node = #7AA0E5 蓝色（一眼区分跟 worker chip）

export interface MentionChipProps {
  agent_id: string;
  role: string;
  role_display: string;
  /** true 时显 ✕ 删除按钮 + 走 onRemove。默认 false。*/
  removable?: boolean;
  onRemove?: () => void;
  /** #60：缺省 worker；node 时强制 #7AA0E5 蓝色。 */
  kind?: MentionKind;
}

export const MentionChip: Component<MentionChipProps> = (props) => {
  const isNode = (): boolean => props.kind === "node";
  const color = (): string => (isNode() ? NODE_CHIP_COLOR : colorForRole(props.role));
  // chip 背景：alpha 0.15（spec §C 写的 rgba(229,165,71,.15) 鲁班例）
  const style = (): JSX.CSSProperties => ({
    "--chip-color": color(),
    "--chip-bg": `${color()}26`, // 26 hex = ~0.15 alpha
    "--chip-border": color(),
  });
  // testid：worker = mention-chip-<agent_id>（旧）；node = mention-chip-node-<node_id>（新）
  const testId = (): string =>
    isNode() ? `mention-chip-node-${props.agent_id}` : `mention-chip-${props.agent_id}`;
  const removeTestId = (): string =>
    isNode()
      ? `mention-chip-remove-node-${props.agent_id}`
      : `mention-chip-remove-${props.agent_id}`;

  return (
    <span
      class={styles.chip}
      style={style()}
      data-testid={testId()}
      data-agent-id={props.agent_id}
      data-role={props.role}
      data-kind={props.kind ?? "worker"}
    >
      <span class={styles.dot} aria-hidden="true" />
      <span class={styles.label}>@{props.role_display}</span>
      <Show when={props.removable}>
        <button
          type="button"
          class={styles.remove}
          aria-label={`移除 @${props.role_display}`}
          data-testid={removeTestId()}
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
