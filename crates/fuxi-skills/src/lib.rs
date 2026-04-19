//! 伏羲点将台（fuxi-skills）—— 玉牒 loader + 招贤流程 + 贤士录。
//!
//! 三个模块围绕同一套目录语义：
//! - `skills/<role>/SKILL.md`           —— 玉牒（已入册）
//! - `skills/<role>.staging/SKILL.md`   —— 榜文（铸牒司产出、待审）
//! - `$HOME/.fuxi/ledger.json`          —— 贤士录（append-only JSON Lines）
//!
//! 为什么要**单独 crate**：v0.1 loader 挤在 fuxi-cli 里，orchestrator 和未来
//! 的 watchers 都要读 SKILL.md——抽成独立 crate 后 cli / orchestrator / memory
//! 都能按需引用，不产生循环依赖。

pub mod ledger;
pub mod loader;
pub mod staging;
pub mod template;

pub use ledger::{LedgerAction, LedgerEntry};
pub use loader::{LoadedSkill, SkillFrontmatter, load, load_from_file, skills_root};
pub use staging::{SkillEntry, SkillState, approve, list_all, reject, stage_write};
