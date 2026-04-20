//! 伏羲点将台（fuxi-skills）—— 玉牒 loader + 招贤流程 + 贤士录。
//!
//! M3.2 改名：`skills/<role>/SKILL.md` → `roles/<role>/ROLE.md`——crate 名
//! 保留 `fuxi-skills`（公共 API 不动，避免 use 站点大面积破坏；v1.2 再考虑 crate rename）。
//!
//! 三个模块围绕同一套目录语义：
//! - `roles/<role>/ROLE.md`             —— 玉牒（已入册）
//! - `roles/<role>.staging/ROLE.md`     —— 榜文（铸牒司产出、待审）
//! - `$HOME/.fuxi/ledger.json`          —— 贤士录（append-only JSON Lines）
//!
//! 读取兼容：loader / staging 仍能识别旧目录 `skills/<role>/SKILL.md`，warn 一行。
//! 用户数据目录 `~/.fuxi/skills/` 由 [`migrate_user_dir`] 启动期一次性 mv 到 roles/。

pub mod ledger;
pub mod loader;
pub mod staging;
pub mod template;

pub use ledger::{LedgerAction, LedgerEntry};
pub use loader::{LoadedSkill, SkillFrontmatter, load, load_from_file, roles_root, skills_root};
pub use staging::{SkillEntry, SkillState, approve, list_all, reject, stage_write};

/// M3.2 迁移工具：把 `~/.fuxi/skills/` 整个 mv 成 `~/.fuxi/roles/`；同时每个
/// 子目录里的 `SKILL.md` 改 `ROLE.md`。
///
/// 幂等：
/// - 目标 `roles/` 已存在 → skip（两者都在时人工处置，本函数不覆盖）
/// - 源 `skills/` 不存在 → 无事发生，返回 Ok(false)
/// - 成功迁移 → 返回 Ok(true)，同时 warn 一行让用户知道
///
/// 失败不 panic，返回 Err 让调用方决定（repl/up 启动时想继续跑就 log 不中断）。
pub fn migrate_user_dir() -> anyhow::Result<bool> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(false);
    };
    let old = std::path::PathBuf::from(&home).join(".fuxi").join("skills");
    let new = std::path::PathBuf::from(&home).join(".fuxi").join("roles");
    if !old.exists() {
        return Ok(false);
    }
    if new.exists() {
        tracing::warn!(
            old = %old.display(),
            new = %new.display(),
            "M3.2 迁移 skip：~/.fuxi/roles 和 ~/.fuxi/skills 两者都存在，请手工合并"
        );
        return Ok(false);
    }
    std::fs::rename(&old, &new).map_err(|e| {
        anyhow::anyhow!("M3.2 迁移失败 ({} → {}): {e}", old.display(), new.display())
    })?;
    // 每个子目录里的 SKILL.md 改 ROLE.md（staging 目录也一视同仁）。
    for entry in std::fs::read_dir(&new)? {
        let Ok(entry) = entry else { continue };
        let skill = entry.path().join("SKILL.md");
        let role_md = entry.path().join("ROLE.md");
        if skill.exists()
            && !role_md.exists()
            && let Err(e) = std::fs::rename(&skill, &role_md)
        {
            tracing::warn!(
                skill = %skill.display(),
                error = %e,
                "M3.2 迁移：单个 SKILL.md → ROLE.md 失败，保留旧名（读取兼容生效）"
            );
        }
    }
    tracing::info!(path = %new.display(), "M3.2 已迁移 ~/.fuxi/skills → ~/.fuxi/roles");
    Ok(true)
}
