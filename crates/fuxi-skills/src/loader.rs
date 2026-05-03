//! 读 `roles/<role>/ROLE.md` → `LoadedSkill`（M3.2 起从 `skills/SKILL.md` 改名）。
//!
//! 用 `serde_yaml` 解析 frontmatter，支持嵌套 `metadata.*`，并把字段抬升成类型
//! `SkillFrontmatter`。body 依旧作为 `append_system_prompt` 透传给门客 CLI。
//!
//! ## 兼容路径
//!
//! M3.2 改名时**保留旧路径为 fallback**：
//! - 优先 `roles/<role>/ROLE.md`
//! - 找不到 → 退到 `skills/<role>/SKILL.md`（warn 一行提示用户 mv）
//!
//! `~/.fuxi/skills/` 的 mv 由 `migrate_user_dir`（启动期一次性）做。

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
    /// CC `--disallowed-tools` 的空格分隔字符串。
    ///
    /// 必要性：`allowed-tools` 在 cc bypassPermissions 模式（fuxi 默认）下不是
    /// 硬白名单——agent 仍能 invoke 不在 list 里的工具。要硬阻断（如玄女不能
    /// 自己 Edit/Write/Task）必须用 disallowed-tools，bypass 模式下仍生效。
    #[serde(rename = "disallowed-tools", default)]
    pub disallowed_tools: Option<String>,
}

/// 加载 Skill 后的结果。
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub profile: AgentProfile,
    pub append_system_prompt: String,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub frontmatter: SkillFrontmatter,
}

/// 找 roles 根目录。查找顺序（每个槽位先找 `roles/` 再 fallback `skills/`）：
/// 1. `FUXI_ROLES_DIR` / `FUXI_SKILLS_DIR` 环境变量
/// 2. 当前 git root 下的 `roles/` → `skills/`
/// 3. cwd 下的 `./roles/` → `./skills/`
/// 4. `$HOME/.fuxi/roles/` → `$HOME/.fuxi/skills/`
pub fn skills_root() -> Option<PathBuf> {
    // env 优先 FUXI_ROLES_DIR，兼容 FUXI_SKILLS_DIR
    for var in ["FUXI_ROLES_DIR", "FUXI_SKILLS_DIR"] {
        if let Ok(p) = std::env::var(var) {
            let path = PathBuf::from(p);
            if path.exists() {
                return Some(path);
            }
        }
    }
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        && output.status.success()
    {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        for name in ["roles", "skills"] {
            let p = PathBuf::from(&root).join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    for name in ["roles", "skills"] {
        let p = PathBuf::from(name);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        for name in ["roles", "skills"] {
            let p = PathBuf::from(&home).join(".fuxi").join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// 新名入口：和 `skills_root` 完全等价（后者保留 API 名以兼容外部 user）。
pub fn roles_root() -> Option<PathBuf> {
    skills_root()
}

/// 读指定 role 的定义文件 → LoadedSkill。
/// 优先找 `<role>/ROLE.md`；找不到 fallback `<role>/SKILL.md`（旧名兼容，warn 一次）。
pub fn load(role: &str) -> Result<LoadedSkill> {
    let root = skills_root().context(
        "找不到 roles 根目录（试过 $FUXI_ROLES_DIR / $FUXI_SKILLS_DIR / git-root/roles / ./roles / ~/.fuxi/roles，及 skills/ 旧名回退）",
    )?;
    let role_md = root.join(role).join("ROLE.md");
    if role_md.exists() {
        return load_from_file(&role_md, role);
    }
    let skill_md = root.join(role).join("SKILL.md");
    if skill_md.exists() {
        tracing::warn!(
            role,
            path = %skill_md.display(),
            "M3.2 旧名 SKILL.md 被读取——请 mv 成 ROLE.md（下 minor 版本删除兼容）"
        );
        return load_from_file(&skill_md, role);
    }
    anyhow::bail!(
        "找不到 role={role} 的定义文件（试过 ROLE.md / SKILL.md 两名，root={})",
        root.display()
    );
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
    let disallowed_tools = fm.disallowed_tools.as_ref().map(|s| {
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

    // CLI 选择来自 frontmatter `metadata.cli`；缺省回 `claude-code`。
    // 取值必须与 `fuxi-orchestrator::WorkerKind::cli_tag` 对齐——daemon 据此
    // 路由到 Cc / Codex / 未来的 Gemini 分支。常见取值："claude-code" / "codex"。
    let cli = fm
        .metadata
        .get("cli")
        .and_then(|v| v.as_str())
        .unwrap_or("claude-code")
        .to_string();

    let profile = AgentProfile {
        name: fm.name.clone(),
        role: role_hint.to_string(),
        cli,
        system_prompt: body.trim().to_string(),
        tags: vec![role_hint.to_string()],
        extra,
    };

    Ok(LoadedSkill {
        profile,
        append_system_prompt: body.trim().to_string(),
        allowed_tools,
        disallowed_tools,
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

    /// ROLE.md 缺 metadata.cli 时回退到 `claude-code`（保留 v0.1 默认行为）。
    #[test]
    fn loader_defaults_cli_to_claude_code_when_metadata_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let role_dir = dir.path().join("luban");
        std::fs::create_dir_all(&role_dir).unwrap();
        let path = role_dir.join("ROLE.md");
        std::fs::write(&path, "---\nname: luban\ndescription: d\n---\nbody\n").unwrap();
        let loaded = load_from_file(&path, "luban").expect("load");
        assert_eq!(loaded.profile.cli, "claude-code");
    }

    /// ROLE.md 写了 `metadata.cli: codex` 时 profile.cli 必须是 codex。
    #[test]
    fn loader_reads_codex_cli_from_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let role_dir = dir.path().join("luban-codex");
        std::fs::create_dir_all(&role_dir).unwrap();
        let path = role_dir.join("ROLE.md");
        std::fs::write(
            &path,
            "---\nname: luban-codex\ndescription: d\nmetadata:\n  cli: codex\n---\nbody\n",
        )
        .unwrap();
        let loaded = load_from_file(&path, "luban-codex").expect("load");
        assert_eq!(loaded.profile.cli, "codex");
    }

    /// `disallowed-tools` frontmatter 字段被 split-whitespace 切碎；
    /// 缺省时 `LoadedSkill.disallowed_tools` 为 None（不禁任何工具）。
    #[test]
    fn loader_parses_disallowed_tools_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let role_dir = dir.path().join("xuannv-test");
        std::fs::create_dir_all(&role_dir).unwrap();
        let path = role_dir.join("ROLE.md");
        std::fs::write(
            &path,
            "---\nname: xuannv-test\ndescription: d\nallowed-tools: Bash(fuxi:*) Read\ndisallowed-tools: Edit Write Task Agent\n---\nbody\n",
        )
        .unwrap();
        let loaded = load_from_file(&path, "xuannv-test").expect("load");
        let got = loaded.disallowed_tools.expect("应解析出 disallowed_tools");
        assert_eq!(
            got,
            vec![
                "Edit".to_string(),
                "Write".to_string(),
                "Task".to_string(),
                "Agent".to_string()
            ],
            "disallowed-tools 应按空白切成 4 项"
        );
    }

    #[test]
    fn loader_disallowed_tools_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let role_dir = dir.path().join("luban-test");
        std::fs::create_dir_all(&role_dir).unwrap();
        let path = role_dir.join("ROLE.md");
        std::fs::write(&path, "---\nname: luban-test\ndescription: d\n---\nbody\n").unwrap();
        let loaded = load_from_file(&path, "luban-test").expect("load");
        assert!(loaded.disallowed_tools.is_none());
    }

    /// M3.2 · 优先 ROLE.md；若同目录**只有** SKILL.md（旧名），也能 load 且 warn。
    #[test]
    fn loader_falls_back_to_legacy_skill_md() {
        // 手工放只有 SKILL.md 的 tempdir，调 load() 走 root 发现路径——
        // 但 load() 会先走 skills_root() 找 ./roles 等，tempdir 不在搜索路径里。
        // 直接用 load_from_file 测 "旧文件名仍能 parse" 足够；search 优先级由
        // loader_prefers_role_md_over_skill_md 覆盖。
        let dir = tempfile::tempdir().expect("tempdir");
        let role_dir = dir.path().join("legacy");
        std::fs::create_dir_all(&role_dir).unwrap();
        let legacy = role_dir.join("SKILL.md");
        std::fs::write(&legacy, "---\nname: legacy\ndescription: d\n---\nbody\n").unwrap();
        let loaded = load_from_file(&legacy, "legacy").expect("旧 SKILL.md 仍可解析");
        assert_eq!(loaded.profile.role, "legacy");
    }

    /// M3.2 search 优先级：同目录两个都有时，`load(role)` 应选 ROLE.md。
    #[test]
    fn loader_prefers_role_md_over_skill_md() {
        let dir = tempfile::tempdir().expect("tempdir");
        // env var 覆盖 search root 到 tempdir；tests 里 unsafe 合法（单线程）。
        unsafe {
            std::env::set_var("FUXI_ROLES_DIR", dir.path());
        }
        let role_dir = dir.path().join("dev");
        std::fs::create_dir_all(&role_dir).unwrap();
        // 旧文件内容 A，新文件内容 B——load 读到 B 才证明优先级对
        std::fs::write(
            role_dir.join("SKILL.md"),
            "---\nname: old-skill\ndescription: old\n---\nold-body\n",
        )
        .unwrap();
        std::fs::write(
            role_dir.join("ROLE.md"),
            "---\nname: new-role\ndescription: new\n---\nnew-body\n",
        )
        .unwrap();

        let loaded = load("dev").expect("load");
        assert_eq!(loaded.profile.name, "new-role");
        assert!(loaded.append_system_prompt.contains("new-body"));
        unsafe {
            std::env::remove_var("FUXI_ROLES_DIR");
        }
    }
}
