import { type Component, type JSX } from "solid-js";
import styles from "./Card.module.css";

// Card · 通用表面卡片（共享原语）。spec §2 质感系统。
// u-card 是 global class（柔影 + 暖光内高光），叠 module class 做色调微染。
export type CardTone = "plain" | "peach" | "lavender" | "mint";

export interface CardProps {
  children: JSX.Element;
  class?: string;
  tone?: CardTone;
}

export const Card: Component<CardProps> = (props) => {
  const tone = (): CardTone => props.tone ?? "plain";
  return (
    <div
      data-testid="ui-card"
      data-tone={tone()}
      // u-card 是 global（非 CSS-module local），手拼到 className 前缀
      class={`u-card ${styles.card} ${props.class ?? ""}`}
    >
      {props.children}
    </div>
  );
};
