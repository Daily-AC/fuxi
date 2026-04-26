import { describe, expect, it } from "vitest";
import { renderMarkdown } from "~/lib/markdown";

describe("renderMarkdown", () => {
  it("inline · bold / italic / code / link", () => {
    const html = renderMarkdown("**粗** *斜* `code` [link](https://example.com)");
    expect(html).toContain("<strong>");
    expect(html).toContain("<em>");
    expect(html).toContain("<code>");
    expect(html).toMatch(/<a [^>]*href="https:\/\/example\.com"/);
  });

  it("link 强制 target=_blank rel=noopener noreferrer", () => {
    const html = renderMarkdown("[x](https://example.com)");
    expect(html).toMatch(/target="_blank"/);
    expect(html).toMatch(/rel="noopener noreferrer"/);
  });

  it("block · heading / list / blockquote / hr / fenced code", () => {
    const md = [
      "# 标题",
      "",
      "- a",
      "- b",
      "",
      "> 引言",
      "",
      "```",
      "let x = 1;",
      "```",
      "",
      "---",
    ].join("\n");
    const html = renderMarkdown(md);
    expect(html).toContain("<h1>");
    expect(html).toContain("<ul>");
    expect(html).toContain("<blockquote>");
    expect(html).toContain("<pre>");
    expect(html).toContain("<code>let x = 1;");
    expect(html).toContain("<hr>");
  });

  it("XSS · script tag 被剔除", () => {
    const html = renderMarkdown("<script>alert(1)</script> hello");
    expect(html).not.toContain("<script");
    expect(html).toContain("hello");
  });

  it("XSS · onerror inline handler 被剔除", () => {
    const html = renderMarkdown('<img src=x onerror="alert(1)">');
    // src=x 可能保留也可能被去；onerror 必须剔
    expect(html).not.toMatch(/onerror=/i);
  });

  it("XSS · javascript: 协议被剔除", () => {
    const html = renderMarkdown("[click](javascript:alert(1))");
    // afterSanitizeAttributes hook 会去除 javascript: href
    expect(html).not.toMatch(/href="javascript:/i);
  });

  it("空字符串 · 返回空", () => {
    expect(renderMarkdown("")).toBe("");
  });

  it("纯文本 · 包成 <p>", () => {
    const html = renderMarkdown("hello world");
    expect(html).toMatch(/<p>hello world/);
  });

  it("代码块 ``` 内容被 escape 不解析", () => {
    const html = renderMarkdown("```\n<script>x</script>\n```");
    expect(html).toMatch(/&lt;script&gt;/);
  });

  it("breaks=true · 单换行变 <br>", () => {
    const html = renderMarkdown("一行\n二行");
    expect(html).toMatch(/<br/);
  });
});
