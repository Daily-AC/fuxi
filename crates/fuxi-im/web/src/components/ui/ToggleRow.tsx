import { Show, type Component } from "solid-js";
import styles from "./ToggleRow.module.css";

// ToggleRow · 设置项 + 开关（共享原语）。左标题/副标题 + 右圆角药丸开关。
// 开关 role=switch，旋钮 spring 滑动；on=薄荷 / off=灰。点开关或整行均 onChange(!checked)。
export interface ToggleRowProps {
  title: string;
  subtitle?: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}

export const ToggleRow: Component<ToggleRowProps> = (props) => {
  const toggle = (): void => props.onChange(!props.checked);
  return (
    <div data-testid="toggle-row" class={`u-card ${styles.row}`} onClick={toggle}>
      <div class={styles.mid}>
        <span class={styles.title}>{props.title}</span>
        <Show when={props.subtitle}>
          <span class={styles.subtitle}>{props.subtitle}</span>
        </Show>
      </div>
      <button
        type="button"
        data-testid="toggle-switch"
        class={styles.switch}
        role="switch"
        aria-checked={props.checked}
        aria-label={props.title}
        onClick={(e) => {
          // 行也响应 toggle —— 阻止冒泡避免双触发
          e.stopPropagation();
          toggle();
        }}
      >
        <span class={styles.knob} aria-hidden="true" />
      </button>
    </div>
  );
};
