import { createMemo, type Component } from "solid-js";
import { renderMarkdown } from "~/lib/markdown";
import styles from "./Markdown.module.css";

// 渲染 sanitized markdown HTML。
// 视觉规则在 .module.css —— inline code 微暗底；pre block surface + radius card；
// blockquote 左 2px accent border；link accent + underline。
export const Markdown: Component<{ source: string; class?: string }> = (props) => {
  const html = createMemo(() => renderMarkdown(props.source));
  return (
    <div
      class={`${styles.md} ${props.class ?? ""}`}
      // sanitize 已在 lib 层完成（DOMPurify allowlist）；innerHTML 安全
      innerHTML={html()}
    />
  );
};
