//! `fuxi memory` 子命令——直接对着 SQLite 文件做 oracle / hetu 操作。
//!
//! 为什么不走 daemon：策府在 v1 阶段只是"让玄女的 Bash 工具有个口"——玄女 skill
//! 会直接 shell 调 `fuxi memory record ...`。走本地文件最简单，也避免 daemon 挂了
//! 玄女就写不了记忆。未来 daemon 里做异步合并器再把这里升级。

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_memory::{HetuStore, NewFact, NewPattern, OracleStore};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, ClapArgs)]
pub struct MemoryArgs {
    /// SQLite 路径。省略时读 `$FUXI_DB`，再省则 `~/.fuxi/memory.db`。
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: MemoryCmd,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCmd {
    /// 查甲骨——按 subject（可选再按 predicate）。
    Query(QueryArgs),
    /// 写一条甲骨事实。
    Record(RecordArgs),
    /// 列河图洛书门客经验（可选按 role 过滤）。
    List(ListArgs),
    /// 全文模糊搜（FTS5）甲骨。
    Search(SearchArgs),
    /// 让新事实覆盖旧事实（老行 valid_until = now）。
    Supersede(SupersedeArgs),
    /// 记录一条门客经验（河图洛书）。
    Learn(LearnArgs),
    /// 把经验标记为已晋升 skill example。
    Promote(PromoteArgs),
}

#[derive(Debug, ClapArgs)]
pub struct QueryArgs {
    #[arg(long)]
    pub subject: String,
    #[arg(long)]
    pub predicate: Option<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: i64,
}

#[derive(Debug, ClapArgs)]
pub struct RecordArgs {
    #[arg(long)]
    pub subject: String,
    #[arg(long)]
    pub predicate: String,
    #[arg(long)]
    pub object: String,
    #[arg(long, default_value = "manual")]
    pub source: String,
    #[arg(long, default_value_t = 0.8)]
    pub confidence: f32,
}

#[derive(Debug, ClapArgs)]
pub struct ListArgs {
    #[arg(long)]
    pub role: Option<String>,
    #[arg(long, default_value = "")]
    pub task_type: String,
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
}

#[derive(Debug, ClapArgs)]
pub struct SearchArgs {
    /// 查询词——中英文混合 OK。
    #[arg(trailing_var_arg = true, required = true)]
    pub query: Vec<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: i64,
}

#[derive(Debug, ClapArgs)]
pub struct SupersedeArgs {
    /// 老事实 id（UUID）。
    #[arg(long = "old")]
    pub old_id: String,
    #[arg(long)]
    pub subject: String,
    #[arg(long)]
    pub predicate: String,
    #[arg(long)]
    pub object: String,
    #[arg(long, default_value = "manual")]
    pub source: String,
    #[arg(long, default_value_t = 0.8)]
    pub confidence: f32,
}

#[derive(Debug, ClapArgs)]
pub struct LearnArgs {
    #[arg(long)]
    pub role: String,
    #[arg(long)]
    pub task_type: String,
    #[arg(long)]
    pub pattern: String,
    #[arg(long, default_value = "success")]
    pub outcome: String,
    #[arg(long, default_value_t = 0.6)]
    pub confidence: f32,
}

#[derive(Debug, ClapArgs)]
pub struct PromoteArgs {
    /// 经验 id（UUID）。
    #[arg(long = "id")]
    pub id: String,
}

pub async fn run(args: MemoryArgs) -> Result<()> {
    let db = resolve_db_path(args.db)?;
    match args.cmd {
        MemoryCmd::Query(a) => run_query(&db, a).await,
        MemoryCmd::Record(a) => run_record(&db, a).await,
        MemoryCmd::List(a) => run_list(&db, a).await,
        MemoryCmd::Search(a) => run_search(&db, a).await,
        MemoryCmd::Supersede(a) => run_supersede(&db, a).await,
        MemoryCmd::Learn(a) => run_learn(&db, a).await,
        MemoryCmd::Promote(a) => run_promote(&db, a).await,
    }
}

/// `--db` > `$FUXI_DB` > `~/.fuxi/memory.db`，父目录不存在则创建。
fn resolve_db_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(env_path) = std::env::var("FUXI_DB")
        && !env_path.is_empty()
    {
        return Ok(PathBuf::from(env_path));
    }
    let home = std::env::var("HOME").context("无法解析 $HOME，请显式 --db 指定 SQLite 路径")?;
    let dir = PathBuf::from(&home).join(".fuxi");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    }
    Ok(dir.join("memory.db"))
}

async fn run_query(db: &PathBuf, args: QueryArgs) -> Result<()> {
    let store = OracleStore::connect_file(db).await?;
    let rows = match args.predicate {
        Some(p) => match store.query_one(&args.subject, &p).await? {
            Some(f) => vec![f],
            None => vec![],
        },
        None => store.query(&args.subject, args.limit).await?,
    };
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}

async fn run_record(db: &PathBuf, args: RecordArgs) -> Result<()> {
    let store = OracleStore::connect_file(db).await?;
    let f = store
        .insert(
            NewFact::new(args.subject, args.predicate, args.object)
                .with_source(args.source)
                .with_confidence(args.confidence),
        )
        .await?;
    println!("{}", serde_json::to_string(&f)?);
    Ok(())
}

async fn run_list(db: &PathBuf, args: ListArgs) -> Result<()> {
    let store = HetuStore::connect_file(db).await?;
    let rows = match args.role {
        Some(r) => store.query(&r, &args.task_type).await?,
        None => store.list_all(args.limit).await?,
    };
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}

async fn run_search(db: &PathBuf, args: SearchArgs) -> Result<()> {
    let q = args.query.join(" ");
    let store = OracleStore::connect_file(db).await?;
    let rows = store.fts_search(&q, args.limit).await?;
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}

async fn run_supersede(db: &PathBuf, args: SupersedeArgs) -> Result<()> {
    let store = OracleStore::connect_file(db).await?;
    let old_id = Uuid::parse_str(&args.old_id)
        .with_context(|| format!("--old 必须是 uuid: {:?}", args.old_id))?;
    let new = store
        .supersede(
            old_id,
            NewFact::new(args.subject, args.predicate, args.object)
                .with_source(args.source)
                .with_confidence(args.confidence),
        )
        .await?;
    println!("{}", serde_json::to_string(&new)?);
    Ok(())
}

async fn run_learn(db: &PathBuf, args: LearnArgs) -> Result<()> {
    let store = HetuStore::connect_file(db).await?;
    let p = store
        .record(
            NewPattern::new(args.role, args.task_type, args.pattern, args.outcome)
                .with_confidence(args.confidence),
        )
        .await?;
    println!("{}", serde_json::to_string(&p)?);
    Ok(())
}

async fn run_promote(db: &PathBuf, args: PromoteArgs) -> Result<()> {
    let store = HetuStore::connect_file(db).await?;
    let id =
        Uuid::parse_str(&args.id).with_context(|| format!("--id 必须是 uuid: {:?}", args.id))?;
    let updated = store.promote(id).await?;
    println!("{}", serde_json::to_string(&updated)?);
    Ok(())
}
