import { type Component } from "solid-js";
import styles from "./Skeleton.module.css";

// Skeleton · 暖奶油底色 + 左→右高光扫光的占位块。spec §5。
export interface SkeletonProps {
  width?: string;
  height?: string;
  radius?: string;
}

export const Skeleton: Component<SkeletonProps> = (props) => {
  return (
    <div
      data-testid="skeleton"
      class={styles.skeleton}
      style={{
        width: props.width ?? "100%",
        height: props.height ?? "16px",
        "border-radius": props.radius ?? "var(--radius-card)",
      }}
    />
  );
};
