//! `fuxi-memory` · 伏羲策府——长期记忆总库。
//!
//! 四层分工（论文 Memory Transfer Learning + `docs/architecture-v1.md §M1.1`）：
//! 1. **甲骨** (`oracle_facts`) —— 玄女长期事实（subject/predicate/object）。
//! 2. **河图洛书** (`hetu_patterns`) —— 门客经验 + insight，可晋升为 skill examples。
//! 3. **身份卡** (`user_profile`) —— Summary 层，spawn 门客时注入 prompt。
//! 4. **简册** —— 复用 `fuxi-events` 的 `events` 表，不在本 crate 新建。
//!
//! 写入策略**只 ADD**（mem0 1.2 思路）：冲突不覆盖，`supersede` 把老行标记
//! `valid_until = now` 再插新行；置信度变化用 `update_confidence(id, delta)`。
//!
//! 检索走 SQLite FTS5（unicode61），< 10k 条 p95 < 10ms，不上向量 / 图。

pub mod error;
pub mod extractor;
pub mod hetu;
pub mod oracle;
pub mod user_profile;

pub use error::{Error, Result};
pub use extractor::{
    DEFAULT_PROMPT_TEMPLATE, EXTRACTOR_ROLE, EXTRACTOR_SOURCE, Extractor, ExtractorConfig,
    FactExtractorSpawner, SpawnerError, SpawnerResult,
};
pub use hetu::{HetuPattern, HetuStore, NewPattern};
pub use oracle::{NewFact, OracleFact, OracleStore};
pub use user_profile::{NewProfile, UserProfileEntry, UserProfileStore};

/// 多份 migration 顺序拼接，编译期嵌入——避免运行期路径依赖，`:memory:` 临时
/// 库也能直接 spin up。新增 migration 只在这里追加文件，**保持 0001 → 0002 → …**
/// 的顺序，schema 演进等于追加 SQL 段。
pub(crate) const SCHEMA_SQL: &str = concat!(
    include_str!("../migrations/0001_memory.sql"),
    "\n",
    include_str!("../migrations/0002_user_profile.sql"),
);

/// 各 store 共用的 schema 初始化——整段 SQL 走 `raw_sql` 一次性执行。
///
/// 为什么不自己按 `;` 切：FTS5 的 `CREATE TRIGGER ... BEGIN ... END;` 体内
/// 有嵌套 `;`，按行切会产出"半条语句"让 SQLite 报 `incomplete input`。
/// sqlx 的 `raw_sql` 正好对应 sqlite3_exec，支持多语句批执行，这才是正道。
///
/// 走完 SCHEMA_SQL 后再调 [`migrate_hetu_to_v2`]——0003_hetu_insight 是 ALTER
/// 不能 IF NOT EXISTS，必须容错跑（重复 init 会撞 duplicate column）。
pub(crate) async fn init_schema(pool: &sqlx::SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA_SQL).execute(pool).await?;
    migrate_hetu_to_v2(pool).await?;
    Ok(())
}

/// 论文 Memory Transfer Learning Insight 层 schema migration。
///
/// 给 `hetu_patterns` 表加 4 列：abstraction_score / derived_from_task / source /
/// valid_until。SQLite ALTER 重复跑撞 "duplicate column name"——所以单条 ALTER
/// 错误**容错 ignore**（除非是其它非预期错），再幂等 CREATE INDEX。
///
/// 为什么不放进 `SCHEMA_SQL` raw_sql：raw_sql 任意一条挂全 batch 失败；
/// 这里每条独立容错，cleaner。
async fn migrate_hetu_to_v2(pool: &sqlx::SqlitePool) -> Result<()> {
    let alters = [
        "ALTER TABLE hetu_patterns ADD COLUMN abstraction_score REAL",
        "ALTER TABLE hetu_patterns ADD COLUMN derived_from_task TEXT",
        "ALTER TABLE hetu_patterns ADD COLUMN source TEXT",
        "ALTER TABLE hetu_patterns ADD COLUMN valid_until TEXT",
    ];
    for sql in alters {
        // duplicate column 错 = 已 migrate 过，安全忽略。其它错（表不存在 / SQLite
        // 内部错）让 init_schema 失败暴露，别吞。
        if let Err(e) = sqlx::query(sql).execute(pool).await {
            let s = format!("{e}");
            if !s.contains("duplicate column") {
                return Err(Error::from(e));
            }
        }
    }
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_hetu_role_active \
         ON hetu_patterns(role) WHERE valid_until IS NULL",
    )
    .execute(pool)
    .await?;
    Ok(())
}
