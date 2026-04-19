//! 招贤的文件系统操作——榜文（staging）⇄ 玉牒（active）⇄ 删除。
//!
//! 约定（点将台布局）：
//! - 玉牒：`<root>/<role>/SKILL.md`
//! - 榜文：`<root>/<role>.staging/SKILL.md`
//!
//! 宗旨：
//! - **approve 必须原子**——用 `rename` 移动整个目录，避免半写。
//! - 如果 `<role>/` 已存在，先挪到 `<role>.bak-<ts>/` 备份，再接榜文。
//! - reject 只删 `.staging`，不触碰 active。

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 玉牒状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillState {
    /// 已入册，可被 spawn。
    Active,
    /// 榜文，待审。
    Staging,
}

/// 点将台的一条记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    pub role: String,
    pub state: SkillState,
    /// SKILL.md 的绝对路径。
    pub path: PathBuf,
}

const STAGING_SUFFIX: &str = ".staging";
const BAK_PREFIX: &str = ".bak-";

/// 写榜文：创建 `<root>/<role>.staging/SKILL.md` 并返回文件路径。
pub fn stage_write(root: &Path, role: &str, content: &str) -> Result<PathBuf> {
    validate_role(role)?;
    let stage_dir = root.join(format!("{role}{STAGING_SUFFIX}"));
    std::fs::create_dir_all(&stage_dir)
        .with_context(|| format!("创建榜文目录 {}", stage_dir.display()))?;
    let path = stage_dir.join("SKILL.md");
    std::fs::write(&path, content).with_context(|| format!("写榜文 {}", path.display()))?;
    Ok(path)
}

/// approve：榜文 rename 成玉牒。若目标已有则先挪成 `<role>.bak-<ts>/`。
pub fn approve(root: &Path, role: &str) -> Result<PathBuf> {
    validate_role(role)?;
    let stage_dir = root.join(format!("{role}{STAGING_SUFFIX}"));
    if !stage_dir.exists() {
        return Err(anyhow!("role={role}: 找不到榜文 {}", stage_dir.display()));
    }
    let active_dir = root.join(role);
    if active_dir.exists() {
        let ts = chrono::Utc::now().timestamp();
        let bak = root.join(format!("{role}{BAK_PREFIX}{ts}"));
        // rename 是同文件系统内原子——把旧 active 让位给即将落户的榜文。
        std::fs::rename(&active_dir, &bak).with_context(|| {
            format!(
                "role={role}: 旧玉牒留档失败（{} → {}）",
                active_dir.display(),
                bak.display()
            )
        })?;
    }
    std::fs::rename(&stage_dir, &active_dir).with_context(|| {
        format!(
            "role={role}: 榜文入册失败（{} → {}）",
            stage_dir.display(),
            active_dir.display()
        )
    })?;
    Ok(active_dir.join("SKILL.md"))
}

/// reject：删榜文目录。没有榜文则报错。
pub fn reject(root: &Path, role: &str) -> Result<()> {
    validate_role(role)?;
    let stage_dir = root.join(format!("{role}{STAGING_SUFFIX}"));
    if !stage_dir.exists() {
        return Err(anyhow!("role={role}: 找不到榜文 {}", stage_dir.display()));
    }
    std::fs::remove_dir_all(&stage_dir)
        .with_context(|| format!("删除榜文 {}", stage_dir.display()))?;
    Ok(())
}

/// 列出 `<root>` 下所有 role（active 或 staging）。忽略 `.bak-*` 备份目录。
pub fn list_all(root: &Path) -> Result<Vec<SkillEntry>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).with_context(|| format!("读目录 {}", root.display()))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if name.starts_with('.') || name.contains(BAK_PREFIX) {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let (role, state) = if let Some(stripped) = name.strip_suffix(STAGING_SUFFIX) {
            (stripped.to_string(), SkillState::Staging)
        } else {
            (name.to_string(), SkillState::Active)
        };
        out.push(SkillEntry {
            role,
            state,
            path: skill_md,
        });
    }
    Ok(out)
}

/// 防止 role 名把 `.staging` / `/` 这种字符注进路径。
fn validate_role(role: &str) -> Result<()> {
    if role.is_empty() {
        return Err(anyhow!("role 不能为空"));
    }
    if role.contains('/')
        || role.contains('\\')
        || role.contains("..")
        || role.starts_with('.')
        || role.ends_with(STAGING_SUFFIX)
    {
        return Err(anyhow!("role 名非法: {role:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_role_rejects_path_traversal() {
        assert!(validate_role("../bad").is_err());
        assert!(validate_role("foo/bar").is_err());
        assert!(validate_role(".hidden").is_err());
        assert!(validate_role("role.staging").is_err());
        assert!(validate_role("").is_err());
        assert!(validate_role("painter").is_ok());
    }
}
