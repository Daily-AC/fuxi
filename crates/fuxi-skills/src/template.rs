//! 铸牒司的填槽工具——把 `templates/<archetype>.archetype.md` 的 `{{xxx}}` 换掉。
//!
//! 约束：
//! - 只接受 `name / description / soul / allowed-tools / generated_at` 五类槽
//! - 遇到未知占位符**直接报错**，不做 silent skip——这样 archetype 写错能早发现

use anyhow::{Result, anyhow};

/// 填槽参数——五个固定字段，对应 archetype 里可出现的占位符。
pub struct RenderArgs<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub soul: &'a str,
    pub allowed_tools: &'a str,
    pub generated_at: &'a str,
}

/// 渲染 archetype 文本。遇到未知 `{{...}}` 报错。
pub fn render(raw: &str, args: &RenderArgs<'_>) -> Result<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(anyhow!("未闭合的占位符 `{{{{`，起始位置={start}"));
        };
        let key = after[..end].trim();
        let replacement = match key {
            "name" => args.name,
            "description" => args.description,
            "soul" => args.soul,
            "allowed-tools" => args.allowed_tools,
            "generated_at" => args.generated_at,
            other => return Err(anyhow!("未知占位符: {{{{{other}}}}}")),
        };
        out.push_str(replacement);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_placeholders_passes_through() {
        let out = render(
            "hello world",
            &RenderArgs {
                name: "",
                description: "",
                soul: "",
                allowed_tools: "",
                generated_at: "",
            },
        )
        .unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn unclosed_placeholder_errors() {
        let err = render(
            "oops {{name",
            &RenderArgs {
                name: "x",
                description: "",
                soul: "",
                allowed_tools: "",
                generated_at: "",
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("未闭合"), "{err}");
    }
}
