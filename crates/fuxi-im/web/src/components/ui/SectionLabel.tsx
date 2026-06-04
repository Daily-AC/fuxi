import { type Component, type JSX } from "solid-js";
import styles from "./SectionLabel.module.css";

// SectionLabel · 小号弱化分区标签（共享原语）。letter-spacing + --text-muted。
export interface SectionLabelProps {
  children: JSX.Element;
}

export const SectionLabel: Component<SectionLabelProps> = (props) => {
  return (
    <div data-testid="section-label" class={styles.label}>
      {props.children}
    </div>
  );
};
