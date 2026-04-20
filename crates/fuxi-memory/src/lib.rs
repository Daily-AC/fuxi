//! `fuxi-memory` · 伏羲策府——长期记忆总库。
//!
//! 三层分工（见 `docs/architecture-v1.md §M1.1` 和 `docs/research/memory-survey.md`）：
//! 1. **甲骨** (`oracle_facts`) —— 玄女长期事实（subject/predicate/object）。
//! 2. **河图洛书** (`hetu_patterns`) —— 门客经验模式，可晋升为 skill examples。
//! 3. **简册** —— 复用 `fuxi-events` 的 `events` 表，不在本 crate 新建。
//!
//! 写入策略**只 ADD**（mem0 1.2 思路）：冲突不覆盖，`supersede` 把老行标记
//! `valid_until = now` 再插新行；置信度变化用 `update_confidence(id, delta)`。
//!
//! 检索走 SQLite FTS5（unicode61），< 10k 条 p95 < 10ms，不上向量 / 图。

pub mod error;
pub mod extractor;
pub mod hetu;
pub mod oracle;

pub use error::{Error, Result};
pub use extractor::{
    DEFAULT_PROMPT_TEMPLATE, EXTRACTOR_ROLE, EXTRACTOR_SOURCE, Extractor, ExtractorConfig,
    FactExtractorSpawner, SpawnerError, SpawnerResult,
};
pub use hetu::{HetuPattern, HetuStore, NewPattern};
pub use oracle::{NewFact, OracleFact, OracleStore};

/// `migrations/0001_memory.sql` 的编译期副本——跟 `fuxi-events` 一样的嵌入策略，
/// 避免运行期路径依赖，`:memory:` 临时库也能跑起来。
pub(crate) const SCHEMA_SQL: &str = include_str!("../migrations/0001_memory.sql");

/// 两个 store 共用 schema 初始化——整段 SQL 走 `raw_sql` 一次性执行。
///
/// 为什么不再自己按 `;` 切：FTS5 的 `CREATE TRIGGER ... BEGIN ... END;` 体内
/// 有嵌套 `;`，按行切会产出"半条语句"让 SQLite 报 `incomplete input`。
/// sqlx 的 `raw_sql` 正好对应 sqlite3_exec，支持多语句批执行，这才是正道。
pub(crate) async fn init_schema(pool: &sqlx::SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA_SQL).execute(pool).await?;
    Ok(())
}
