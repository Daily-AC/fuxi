//! template 填槽：把 archetype 骨架的 `{{placeholder}}` 替换成实参。

use fuxi_skills::template::{RenderArgs, render};

#[test]
fn render_substitutes_all_placeholders() {
    let raw = "---\n\
        name: {{name}}\n\
        description: {{description}}\n\
        allowed-tools: {{allowed-tools}}\n\
        metadata:\n  \
          generated_at: {{generated_at}}\n\
        ---\n\n\
        # {{name}}\n\n{{soul}}\n";

    let out = render(
        raw,
        &RenderArgs {
            name: "painter",
            description: "画图门客",
            soul: "你是画图门客",
            allowed_tools: "Read Write Bash",
            generated_at: "2026-04-19T12:00:00Z",
        },
    )
    .expect("render");

    assert!(out.contains("name: painter"));
    assert!(out.contains("description: 画图门客"));
    assert!(out.contains("allowed-tools: Read Write Bash"));
    assert!(out.contains("generated_at: 2026-04-19T12:00:00Z"));
    assert!(out.contains("# painter"));
    assert!(out.contains("你是画图门客"));
    // 无残留占位符
    assert!(!out.contains("{{"), "render 后仍有占位符:\n{out}");
}

#[test]
fn render_errors_on_unknown_placeholder() {
    let raw = "hi {{strange}}";
    let err = render(
        raw,
        &RenderArgs {
            name: "x",
            description: "x",
            soul: "x",
            allowed_tools: "x",
            generated_at: "x",
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("strange"), "实际: {err}");
}
