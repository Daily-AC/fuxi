import { For, Show, createEffect, createMemo, onCleanup, onMount, type Component } from "solid-js";
import { colorForRole } from "~/tokens";
import { NODE_CHIP_COLOR, PROJECT_CHIP_COLOR, type MentionCandidate } from "~/lib/mentions";
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

  // #60 + v2 跨节点：三段排序——worker → project → node。保留原 props.candidates
  // 顺序但分三段，三段交界处渲染 separator label（"项目" / "节点"）。
  const ordered = createMemo<MentionCandidate[]>(() => {
    const workers = props.candidates.filter((c) => c.kind !== "node" && c.kind !== "project");
    const projects = props.candidates.filter((c) => c.kind === "project");
    const nodes = props.candidates.filter((c) => c.kind === "node");
    return [...workers, ...projects, ...nodes];
  });
  // project / node 段的起始 index，用于渲染 separator
  const firstProjectIdx = createMemo<number>(() => {
    const list = ordered();
    return list.findIndex((c) => c.kind === "project");
  });
  const firstNodeIdx = createMemo<number>(() => {
    const list = ordered();
    return list.findIndex((c) => c.kind === "node");
  });

  return (
    <Show when={props.visible}>
      <div
        ref={popupRef}
        class={styles.popup}
        data-testid="mention-popup"
        role="listbox"
        aria-label="选择门客或节点"
      >
        <Show
          when={ordered().length > 0}
          fallback={
            <div class={styles.empty} data-testid="mention-popup-empty">
              没找到匹配的门客
            </div>
          }
        >
          <For each={ordered()}>
            {(c, i) => {
              const isActive = (): boolean => i() === props.highlightedIndex;
              const isNode = (): boolean => c.kind === "node";
              const isProject = (): boolean => c.kind === "project";
              const dotStyle = (): { background: string } => ({
                background: isNode()
                  ? NODE_CHIP_COLOR
                  : isProject()
                    ? PROJECT_CHIP_COLOR
                    : colorForRole(c.role),
              });
              const itemTestId = (): string => {
                if (isNode()) return `mention-item-node-${c.agent_id}`;
                if (isProject()) return `mention-item-project-${c.agent_id}`;
                return `mention-item-${c.agent_id}`;
              };
              const showProjectSep = (): boolean =>
                isProject() && i() === firstProjectIdx() && i() > 0;
              const showNodeSep = (): boolean => isNode() && i() === firstNodeIdx() && i() > 0;
              return (
                <>
                  <Show when={showProjectSep()}>
                    <div
                      class={styles.separator}
                      data-testid="mention-popup-project-sep"
                      aria-hidden="true"
                    >
                      项目
                    </div>
                  </Show>
                  <Show when={showNodeSep()}>
                    <div
                      class={styles.separator}
                      data-testid="mention-popup-node-sep"
                      aria-hidden="true"
                    >
                      节点
                    </div>
                  </Show>
                  <button
                    type="button"
                    class={styles.item}
                    classList={{ [styles.itemActive ?? ""]: isActive() }}
                    role="option"
                    aria-selected={isActive()}
                    data-testid={itemTestId()}
                    data-mention-idx={i()}
                    data-kind={c.kind ?? "worker"}
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
                </>
              );
            }}
          </For>
        </Show>
      </div>
    </Show>
  );
};
