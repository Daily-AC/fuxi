import { For, Show, createEffect, onCleanup, onMount, type Component } from "solid-js";
import { colorForRole } from "~/tokens";
import type { MentionCandidate } from "~/lib/mentions";
import styles from "./MentionAutocomplete.module.css";

// MentionAutocomplete · v3 #N2' / #37
// 设计 spec: 2026-04-26-im-tab-bar-task-thread-design.md §autocomplete 弹层
//
// 单选 list popup，紧贴 composer 上方 absolute 定位（外层 composer 必须 position:relative）。
// 行为：
//   - 候选项 ≥ 44px 触控
//   - 上下键 + Enter 选中（document keydown 全局监听，仅 visible=true 时 active）
//   - Esc 调 onCancel
//   - tap 候选项 → onSelect
//   - candidates 为空时显空态文案 + onCancel 不自动调（让外层 composer 决定关弹层）
// 受控：parent 持 highlightedIndex；按键 → onMoveSelection(±1)。

export interface MentionAutocompleteProps {
  /** 显示与否；false 时不渲染。*/
  visible: boolean;
  candidates: MentionCandidate[];
  highlightedIndex: number;
  onSelect: (c: MentionCandidate) => void;
  onCancel: () => void;
  onMoveSelection: (delta: 1 | -1) => void;
}

export const MentionAutocomplete: Component<MentionAutocompleteProps> = (props) => {
  let popupRef: HTMLDivElement | undefined;

  const onKeyDown = (e: KeyboardEvent): void => {
    if (!props.visible) return;
    if (e.key === "Escape") {
      e.preventDefault();
      props.onCancel();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      props.onMoveSelection(1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      props.onMoveSelection(-1);
      return;
    }
    if (e.key === "Enter") {
      // Enter 选中候选 · 没候选 noop（让外层 composer 决定怎么处理 Enter）
      if (props.candidates.length === 0) return;
      const i = Math.max(0, Math.min(props.highlightedIndex, props.candidates.length - 1));
      const c = props.candidates[i];
      if (!c) return;
      e.preventDefault();
      props.onSelect(c);
    }
  };

  onMount(() => document.addEventListener("keydown", onKeyDown));
  onCleanup(() => document.removeEventListener("keydown", onKeyDown));

  // 高亮项滚入视口
  createEffect(() => {
    if (!props.visible || !popupRef) return;
    const i = props.highlightedIndex;
    const item = popupRef.querySelector<HTMLElement>(`[data-mention-idx="${i}"]`);
    if (item && typeof item.scrollIntoView === "function") {
      item.scrollIntoView({ block: "nearest" });
    }
  });

  return (
    <Show when={props.visible}>
      <div
        ref={popupRef}
        class={styles.popup}
        data-testid="mention-popup"
        role="listbox"
        aria-label="选择门客"
      >
        <Show
          when={props.candidates.length > 0}
          fallback={
            <div class={styles.empty} data-testid="mention-popup-empty">
              没找到匹配的门客
            </div>
          }
        >
          <For each={props.candidates}>
            {(c, i) => {
              const isActive = (): boolean => i() === props.highlightedIndex;
              const dotStyle = (): { background: string } => ({
                background: colorForRole(c.role),
              });
              return (
                <button
                  type="button"
                  class={styles.item}
                  classList={{ [styles.itemActive ?? ""]: isActive() }}
                  role="option"
                  aria-selected={isActive()}
                  data-testid={`mention-item-${c.agent_id}`}
                  data-mention-idx={i()}
                  onClick={(e) => {
                    e.preventDefault();
                    props.onSelect(c);
                  }}
                >
                  <span class={styles.dot} style={dotStyle()} aria-hidden="true" />
                  <span class={styles.role}>{c.role_display}</span>
                  <Show when={c.hint}>
                    <span class={styles.hint}>· {c.hint}</span>
                  </Show>
                </button>
              );
            }}
          </For>
        </Show>
      </div>
    </Show>
  );
};
