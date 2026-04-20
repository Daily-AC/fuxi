//! 招贤的文件系统操作——榜文（staging）⇄ 玉牒（active）⇄ 删除。
//!
//! 约定（点将台布局）（M3.2 起：旧 SKILL.md 改 ROLE.md）：
//! - 玉牒：`<root>/<role>/ROLE.md`
//! - 榜文：`<root>/<role>.staging/ROLE.md`
//! - 兼容：`list_all` 也接纳仍叫 SKILL.md 的旧目录（warn），便于渐进迁移
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
    /// role 定义文件的绝对路径（通常是 `ROLE.md`，旧目录可能仍是 `SKILL.md`）。
    pub path: PathBuf,
}

const STAGING_SUFFIX: &str = ".staging";
const BAK_PREFIX: &str = ".bak-";
/// 新名——M3.2 起 `ROLE.md`；`SKILL.md` 是读取兼容保留。
pub(crate) const ROLE_FILE: &str = "ROLE.md";
/// 旧名——仅在读取路径兜底，写都走 `ROLE_FILE`。
pub(crate) const LEGACY_SKILL_FILE: &str = "SKILL.md";

/// 写榜文：创建 `<root>/<role>.staging/ROLE.md` 并返回文件路径。
pub fn stage_write(root: &Path, role: &str, content: &str) -> Result<PathBuf> {
    validate_role(role)?;
    let stage_dir = root.join(format!("{role}{STAGING_SUFFIX}"));
    std::fs::create_dir_all(&stage_dir)
        .with_context(|| format!("创建榜文目录 {}", stage_dir.display()))?;
    let path = stage_dir.join(ROLE_FILE);
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
    // 新 approve 落盘用 ROLE.md；若目录里仍是旧 SKILL.md（不该发生——stage_write 已写 ROLE.md），
    // 返回实际在的那个。
    let role_md = active_dir.join(ROLE_FILE);
    if role_md.exists() {
        Ok(role_md)
    } else {
        Ok(active_dir.join(LEGACY_SKILL_FILE))
    }
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
        // M3.2 · 优先 ROLE.md，fallback SKILL.md（兼容未迁移的旧目录）
        let role_file = entry.path().join(ROLE_FILE);
        let path = if role_file.exists() {
            role_file
        } else {
            let legacy = entry.path().join(LEGACY_SKILL_FILE);
            if !legacy.exists() {
                continue;
            }
            legacy
        };
        let (role, state) = if let Some(stripped) = name.strip_suffix(STAGING_SUFFIX) {
            (stripped.to_string(), SkillState::Staging)
        } else {
            (name.to_string(), SkillState::Active)
        };
        out.push(SkillEntry { role, state, path });
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
