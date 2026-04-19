//! 读 `skills/<role>/SKILL.md` → `LoadedSkill`。
//!
//! 相比 v0.1 `fuxi-cli` 里的最简实现，这里用 `serde_yaml` 解析 frontmatter，
//! 支持嵌套 `metadata.*`，并把字段抬升成类型 `SkillFrontmatter`。body 依旧作为
//! `append_system_prompt` 透传给门客 CLI。

use anyhow::{Context, Result};
use fuxi_core::agent::AgentProfile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// SKILL.md 的 YAML frontmatter——agentskills.io 兼容字段 + 嵌套 metadata。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillFrontmatter {
    /// 必填——role 名称（ASCII lowercase / hyphen）。
    #[serde(default)]
    pub name: String,
    /// 必填——供玄女 match 时读的一句话。
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    /// 嵌套自由字段，保留原始 YAML 结构（运行期再挑需要的）。
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_yaml::Value>,
    /// CC `--allowed-tools` 的空格分隔字符串——保留原字符串，用 `allowed_tools()` 切。
    #[serde(rename = "allowed-tools", default)]
    pub allowed_tools: Option<String>,
}

/// 加载 Skill 后的结果。
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub profile: AgentProfile,
    pub append_system_prompt: String,
    pub allowed_tools: Option<Vec<String>>,
    pub frontmatter: SkillFrontmatter,
}

/// 找 skills 根目录。查找顺序：
/// 1. `FUXI_SKILLS_DIR` 环境变量
/// 2. 当前 git root 下的 `skills/`
/// 3. cwd 下的 `./skills/`
/// 4. `$HOME/.fuxi/skills/`
pub fn skills_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FUXI_SKILLS_DIR") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        && output.status.success()
    {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let p = PathBuf::from(root).join("skills");
        if p.exists() {
            return Some(p);
        }
    }
    let cwd_skills = PathBuf::from("skills");
    if cwd_skills.exists() {
        return Some(cwd_skills);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".fuxi").join("skills");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 读指定 role 的 SKILL.md → LoadedSkill。
pub fn load(role: &str) -> Result<LoadedSkill> {
    let root = skills_root()
        .context("找不到 skills 根目录（试过 $FUXI_SKILLS_DIR / git-root/skills / ./skills / ~/.fuxi/skills）")?;
    let path = root.join(role).join("SKILL.md");
    load_from_file(&path, role)
}

/// 给定路径 + role 提示加载。测试 / 非标准位置用这个。
pub fn load_from_file(path: &Path, role_hint: &str) -> Result<LoadedSkill> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("读取 {}", path.display()))?;
    let (fm_text, body) = split_frontmatter(&content);
    let mut fm: SkillFrontmatter = if fm_text.is_empty() {
        SkillFrontmatter::default()
    } else {
        serde_yaml::from_str(fm_text)
            .with_context(|| format!("解析 frontmatter（YAML）: {}", path.display()))?
    };
    // name 缺就兜 role_hint——保持 v0.1 行为一致。
    if fm.name.is_empty() {
        fm.name = role_hint.to_string();
    }

    let allowed_tools = fm.allowed_tools.as_ref().map(|s| {
        s.split_whitespace()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
    });

    let mut extra = BTreeMap::new();
    if !fm.description.is_empty() {
        extra.insert(
            "description".to_string(),
            serde_json::Value::String(fm.description.clone()),
        );
    }

    let profile = AgentProfile {
        name: fm.name.clone(),
        role: role_hint.to_string(),
        cli: "claude-code".to_string(),
        system_prompt: body.trim().to_string(),
        tags: vec![role_hint.to_string()],
        extra,
    };

    Ok(LoadedSkill {
        profile,
        append_system_prompt: body.trim().to_string(),
        allowed_tools,
        frontmatter: fm,
    })
}

/// 把 markdown 按 `---\n…\n---\n…` 切成 (frontmatter, body)。
/// 无 frontmatter 时返回 `("", full_content)`。
fn split_frontmatter(content: &str) -> (&str, &str) {
    if !content.starts_with("---\n") {
        return ("", content);
    }
    let after_open = &content[4..];
    let Some((fm, after_close)) = after_open.split_once("\n---") else {
        return ("", content);
    };
    let body = after_close.strip_prefix('\n').unwrap_or(after_close);
    (fm, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_extracts_yaml_block() {
        let content = "---\nname: test\n---\nhello\n";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("name: test"));
        assert_eq!(body.trim(), "hello");
    }

    #[test]
    fn split_frontmatter_handles_no_fm() {
        let content = "no fm here";
        let (fm, body) = split_frontmatter(content);
        assert_eq!(fm, "");
        assert_eq!(body, "no fm here");
    }
}
