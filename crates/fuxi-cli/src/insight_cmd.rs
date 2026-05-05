//! `fuxi insight` —— 河图洛书 insight 层（论文 Memory Transfer Learning）的查 / 写入口。
//!
//! 跟 `fuxi memory learn`（v1 招贤司路径）的区别：
//! - `memory learn` 走 [`NewPattern::new`] 三元组（task_type 必填，给单 task 经验用）
//! - `insight record` 走 [`NewPattern::insight`] 默认 task-agnostic（task_type=""）+
//!   source="manual" / "cangjie-auto"
//!
//! 玄女**只读**这层（`fuxi insight list`），写入由仓颉自动做（`source=cangjie-auto`）。
//! 此 CLI 暴露 `record` 子命令是给玄女**手动**入心法用——比如用户自己说"门客 X 这种
//! 活记一下经验"。
//!
//! db 路径同 `fuxi profile`：默认 `$HOME/.fuxi/events.db`（同库不双写）。

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_memory::{HetuStore, NewPattern};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, ClapArgs)]
pub struct InsightArgs {
    /// SQLite 路径覆写。省略走 `$HOME/.fuxi/events.db`——同 fuxi profile / fuxi note。
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: InsightCmd,
}

#[derive(Debug, Subcommand)]
pub enum InsightCmd {
    /// 列 active hetu_patterns。可按 role 过滤，用 abstraction + 时间排序。
    List(ListArgs),
    /// 手动入一条心法（玄女判断后决定写入；自动入由仓颉做）。
    Record(RecordArgs),
    /// 把指定 id 的心法标过期（valid_until=now，行保留供审计）。
    Supersede(SupersedeArgs),
}

#[derive(Debug, ClapArgs)]
pub struct ListArgs {
    /// 按 role 过滤——空则列全表 active。给 role 时走 `recent_for_role` 走
    /// 抽象度优先排序（论文核心）。
    #[arg(long)]
    pub role: Option<String>,
    /// 上限。默认 50，对玄女 prompt 注入足够（注入桥只取前 5）。
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
}

#[derive(Debug, ClapArgs)]
pub struct RecordArgs {
    /// 哪个角色的心法——`luban` / `luban-codex` / ...（不能空）。
    #[arg(long)]
    pub role: String,
    /// 任务类别。空串 = task-agnostic insight（论文常见形态——抽象度高的心法
    /// 跨 task type 都能迁移）。
    #[arg(long, default_value = "")]
    pub task_type: String,
    /// 心法正文（自然语言一两句话）。位置参数——剩余 token 拼起来。
    #[arg(trailing_var_arg = true, required = true)]
    pub text: Vec<String>,
    /// 来源标记。默认 `manual`——手动入；自动提取走 `cangjie-auto`（仓颉触发器）。
    #[arg(long, default_value = "manual")]
    pub source: String,
    /// 抽象度评分 0.0-1.0（可选）。手动入一般不填，让排序按时间走。
    #[arg(long)]
    pub abstraction_score: Option<f64>,
    /// 置信度 0.0-1.0。默认 0.6——手动入比 cangjie 自动 0.5 略高（人判断更可信）。
    #[arg(long, default_value_t = 0.6)]
    pub confidence: f32,
}

#[derive(Debug, ClapArgs)]
pub struct SupersedeArgs {
    /// 心法 id（uuid）。
    pub id: String,
}

pub async fn run(args: InsightArgs) -> Result<()> {
    let db = resolve_db_path(args.db)?;
    match args.cmd {
        InsightCmd::List(a) => run_list(&db, a).await,
        InsightCmd::Record(a) => run_record(&db, a).await,
        InsightCmd::Supersede(a) => run_supersede(&db, a).await,
    }
}

/// 同 profile_cmd——钉死 events.db，让 cangjie/玄女在同一份 SQLite 里看到全部。
fn resolve_db_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    let home = std::env::var("HOME").context("无法解析 $HOME，请显式 --db 指定 SQLite 路径")?;
    let dir = PathBuf::from(&home).join(".fuxi");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).with_context(|| format!("创建 {} 失败", dir.display()))?;
    }
    Ok(dir.join("events.db"))
}

async fn run_list(db: &PathBuf, args: ListArgs) -> Result<()> {
    let store = HetuStore::connect_file(db).await?;
    let rows = match args.role.as_deref() {
        Some(r) => store.recent_for_role(r, args.limit as usize).await?,
        None => store.list_active(args.limit).await?,
    };
    if rows.is_empty() {
        eprintln!("当前没有 active 心法。");
        println!("[]");
        return Ok(());
    }
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}

async fn run_record(db: &PathBuf, args: RecordArgs) -> Result<()> {
    if args.role.trim().is_empty() {
        anyhow::bail!("--role 不能为空");
    }
    let text = args.text.join(" ");
    if text.trim().is_empty() {
        anyhow::bail!("心法正文不能为空");
    }
    let mut new = NewPattern::insight(args.role.clone(), text)
        .with_task_type(args.task_type)
        .with_source(args.source)
        .with_confidence(args.confidence);
    if let Some(s) = args.abstraction_score {
        new = new.with_abstraction_score(s);
    }
    let store = HetuStore::connect_file(db).await?;
    let p = store.record(new).await?;
    println!("{}", serde_json::to_string(&p)?);
    Ok(())
}

async fn run_supersede(db: &PathBuf, args: SupersedeArgs) -> Result<()> {
    let id = Uuid::parse_str(&args.id).with_context(|| format!("id 必须是 uuid: {:?}", args.id))?;
    let store = HetuStore::connect_file(db).await?;
    store.supersede(id).await?;
    println!("{}", serde_json::json!({"superseded": id.to_string()}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 集成测：手动 record + list 走完整 store 路径，覆盖 source/abstraction_score 默认。
    #[tokio::test]
    async fn record_then_list_active() {
        let store = HetuStore::connect_memory().await.unwrap();
        // 仿 run_record 的拼装
        let new = NewPattern::insight("luban", "TDD 红绿循环显著降低 false-positive")
            .with_task_type("")
            .with_source("manual")
            .with_confidence(0.6)
            .with_abstraction_score(0.85);
        let p = store.record(new).await.unwrap();
        assert_eq!(p.role, "luban");
        assert_eq!(p.source, "manual");
        assert_eq!(p.abstraction_score, Some(0.85));

        let active = store.list_active(10).await.unwrap();
        assert_eq!(active.len(), 1);
    }

    /// recent_for_role 高抽象度优先（spawn 注入桥就靠这个排序）。
    #[tokio::test]
    async fn recent_for_role_prefers_high_abstraction() {
        let store = HetuStore::connect_memory().await.unwrap();
        store
            .record(NewPattern::insight("luban", "low").with_abstraction_score(0.3))
            .await
            .unwrap();
        store
            .record(NewPattern::insight("luban", "high").with_abstraction_score(0.9))
            .await
            .unwrap();
        let rows = store.recent_for_role("luban", 5).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pattern, "high");
        assert_eq!(rows[1].pattern, "low");
    }

    /// supersede 后 active 视图清掉。
    #[tokio::test]
    async fn supersede_removes_from_active() {
        let store = HetuStore::connect_memory().await.unwrap();
        let p = store
            .record(NewPattern::insight("luban", "expire me"))
            .await
            .unwrap();
        store.supersede(p.id).await.unwrap();
        let rows = store.list_active(10).await.unwrap();
        assert!(rows.is_empty());
    }
}
