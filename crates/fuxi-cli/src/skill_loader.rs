//! 读 `skills/<role>/SKILL.md` → `AgentProfile` + cc 启动参数。
//!
//! v0.1 最小实现：
//! - Frontmatter 用 `---\n...\n---` 分隔 YAML-like key:value 简写；
//! - body 作为 `append_system_prompt` 注入给 cc；
//! - `allowed-tools` 字符串按空格切，透传给 cc `--allowed-tools`。
//!
//! 完整 agentskills spec 的支持（scripts/ resources/ metadata 等）延到 v0.1.5+。
//! 这里够跑通 v0.1 场景就行。

use anyhow::{Context, Result};
use fuxi_core::agent::AgentProfile;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 加载 Skill 后的结果——分离 profile (给 orchestrator) 和 cc config
/// hint（给 CcLaunchConfig.append_system_prompt 用）。
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub profile: AgentProfile,
    pub append_system_prompt: String,
    pub allowed_tools: Option<Vec<String>>,
}

/// 找 skills 根目录。查找顺序：
/// 1. `FUXI_SKILLS_DIR` 环境变量
/// 2. 当前 git root 下的 `skills/`（开发中常用）
/// 3. 仓库内的 `./skills/`（cwd）
pub fn skills_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FUXI_SKILLS_DIR") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    // git root
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
    // cwd
    let cwd_skills = PathBuf::from("skills");
    if cwd_skills.exists() {
        return Some(cwd_skills);
    }
    None
}

/// 读指定 role 的 SKILL.md → LoadedSkill。
pub fn load(role: &str) -> Result<LoadedSkill> {
    let root = skills_root()
        .context("找不到 skills 根目录（试过 $FUXI_SKILLS_DIR / git-root/skills / ./skills）")?;
    let path = root.join(role).join("SKILL.md");
    load_from_file(&path, role)
}

fn load_from_file(path: &Path, role_hint: &str) -> Result<LoadedSkill> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("读取 {}", path.display()))?;
    let (fm, body) = split_frontmatter(&content);
    let meta = parse_kv_frontmatter(fm);

    // name 优先走 frontmatter，兜底 role_hint
    let name = meta
        .get("name")
        .cloned()
        .unwrap_or_else(|| role_hint.to_string());
    let description = meta.get("description").cloned().unwrap_or_default();
    let allowed_tools = meta.get("allowed-tools").map(|s| {
        s.split_whitespace()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
    });

    let profile = AgentProfile {
        name: name.clone(),
        role: role_hint.to_string(),
        cli: "claude-code".to_string(),
        system_prompt: body.trim().to_string(),
        tags: vec![role_hint.to_string()],
        extra: {
            let mut m = BTreeMap::new();
            if !description.is_empty() {
                m.insert(
                    "description".to_string(),
                    serde_json::Value::String(description),
                );
            }
            m
        },
    };

    Ok(LoadedSkill {
        profile,
        append_system_prompt: body.trim().to_string(),
        allowed_tools,
    })
}

/// 把 markdown 按 `---\n…\n---\n…` 切成 (frontmatter, body)。
/// 无 frontmatter 时返回 `("", full_content)`。
fn split_frontmatter(content: &str) -> (&str, &str) {
    if !content.starts_with("---\n") {
        return ("", content);
    }
    let after_open = &content[4..]; // 越过首行 "---\n"
    let Some((fm, after_close)) = after_open.split_once("\n---") else {
        return ("", content);
    };
    // after_close 紧跟在闭合 `---` 之后；正常 markdown 下是 `\n<body>`。
    let body = after_close.strip_prefix('\n').unwrap_or(after_close);
    (fm, body)
}

/// 极简 YAML-like：`key: value`，忽略空行和以 `#` 开头的注释行。不支持嵌套。
fn parse_kv_frontmatter(fm: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in fm.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_string();
            let mut value = v.trim().to_string();
            // 去掉两侧的单/双引号
            if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value = value[1..value.len() - 1].to_string();
            }
            map.insert(key, value);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn split_frontmatter_extracts_yaml_block() {
        let content = "---\nname: test\ndescription: hi\n---\nhello body\n";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("name: test"));
        assert_eq!(body.trim(), "hello body");
    }

    #[test]
    fn split_frontmatter_no_frontmatter_returns_empty_fm() {
        let content = "just body";
        let (fm, body) = split_frontmatter(content);
        assert_eq!(fm, "");
        assert_eq!(body, "just body");
    }

    #[test]
    fn parse_kv_strips_quotes_and_comments() {
        let fm = "name: xuannv\n# comment\ndescription: \"some thing\"\n";
        let map = parse_kv_frontmatter(fm);
        assert_eq!(map.get("name").map(String::as_str), Some("xuannv"));
        assert_eq!(
            map.get("description").map(String::as_str),
            Some("some thing")
        );
    }

    #[test]
    fn allowed_tools_is_space_separated() {
        let fm = "name: x\nallowed-tools: Bash(git:*) Read Edit\n";
        let map = parse_kv_frontmatter(fm);
        let tools = map.get("allowed-tools").unwrap();
        let parts: Vec<_> = tools.split_whitespace().collect();
        assert_eq!(parts, vec!["Bash(git:*)", "Read", "Edit"]);
    }

    #[test]
    fn load_from_file_end_to_end_with_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("dev");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "---\nname: dev\ndescription: the developer gate\nallowed-tools: Read Edit Bash\n---\n\nYou are the dev worker."
        )
        .unwrap();
        drop(f);

        let loaded = load_from_file(&path, "dev").expect("load");
        assert_eq!(loaded.profile.name, "dev");
        assert_eq!(loaded.profile.role, "dev");
        assert!(loaded.append_system_prompt.contains("dev worker"));
        let tools = loaded.allowed_tools.unwrap();
        assert_eq!(tools, vec!["Read", "Edit", "Bash"]);
    }
}
