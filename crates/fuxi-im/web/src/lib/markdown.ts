// markdown 渲染：marked → DOMPurify sanitize。
// 强制 GFM、breaks、target=_blank for links；只允许保守的 tag/attr 集合。
//
// 关键：streaming 时每 delta 都重 parse → 重 sanitize → 重 set innerHTML。
// 单条消息体积小，开销可忽略；外部按消息 id 做 memoize 也行（v1 不上）。

import DOMPurify from "dompurify";
import { marked } from "marked";

// marked 配置 —— 一次性，全局共享。
marked.setOptions({
  gfm: true,
  breaks: true, // 单换行 → <br>，符合 IM 用户预期
});

// 链接渲染：所有 a 标签强制 target=_blank rel=noopener noreferrer。
// 注：marked 15.x extension API 改为 useRenderer/Tokenizer；这里 hook 简单的 walkTokens
// 不可靠，转用 DOMPurify 阶段统一加 attr。

const ALLOWED_TAGS = [
  "p",
  "br",
  "strong",
  "em",
  "del",
  "code",
  "pre",
  "blockquote",
  "ul",
  "ol",
  "li",
  "a",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "span",
  "img",
];

const ALLOWED_ATTR = ["href", "title", "target", "rel", "src", "alt"];

// 用 hook 给 DOMPurify 处理 a 标签：强制 target=_blank rel；丢弃 javascript: 协议。
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (!(node instanceof Element)) return;
  if (node.tagName === "A") {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
    const href = node.getAttribute("href");
    if (href && /^javascript:/i.test(href)) node.removeAttribute("href");
  }
});

/** parse + sanitize markdown，输出可直接 innerHTML 的 string。
 *  sync 入口（marked 默认是 sync）；如果上游 await，async 也兼容。*/
export function renderMarkdown(src: string): string {
  if (!src) return "";
  // marked.parse 同步，{ async: false } 显式锁定
  const html = marked.parse(src, { async: false }) as string;
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    USE_PROFILES: { html: true },
  });
}
