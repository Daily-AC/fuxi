//! Phase 1 · `fuxi topic` 子命令家族。
//!
//! 四个动作：
//! - `fuxi topic new <title>` — 在 im.db 建一个新 topic，stdout 输出 uuid
//! - `fuxi topic list [--include-archived] [--json]` — 列 topic
//! - `fuxi topic switch <id|title>` — 通过 daemon ipc 切玄女当前 topic（Phase 2 常驻分身秒切）
//! - `fuxi topic archive <id|title>` — 把 topic 设归档（不删消息）
//!
//! `new/list/archive` 直接操 ~/.fuxi/im.db（同 issue_cmd / bug_cmd pattern）。
//! `switch` 走 daemon ipc，因为需要在线 fuxi 进程操作分身池（懒启动/秒切）。
//! daemon 不在线时 switch 报"daemon 未运行"。
//!
//! `id|title` 二选一解析：
//! - 32 位 UUID → 直接当 TopicId
//! - 8 字符 UUID 前缀 → SQL LIKE 匹配（unique 时）
//! - 其他 → 当 title 字面量在 topics 表找

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use fuxi_core::TopicId;
use fuxi_im::db as im_db;
use fuxi_im::topic_store::TopicStore;
use uuid::Uuid;

#[derive(Debug, Args)]
pub struct TopicArgs {
    #[command(subcommand)]
    pub cmd: TopicCmd,
}

#[derive(Debug, Subcommand)]
pub enum TopicCmd {
    /// 建一个新 topic。stdout 输出新 topic 的 uuid（机器/玄女可拿去 switch）。
    New(NewArgs),
    /// 列 topic。默认只列活跃；--include-archived 一并列归档。
    List(ListArgs),
    /// 切到某 topic——常驻分身秒切；池中无活分身才懒启动（topic 回顾 prelude）。
    Switch(SwitchArgs),
    /// 归档 topic（不删消息；sidebar 默认不显）。
    Archive(ArchiveArgs),
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// topic 标题，用户可见、可改、可重复。
    pub title: String,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// 一并显示归档 topic。
    #[arg(long)]
    pub include_archived: bool,
    /// JSON 数组输出（机器可读）。
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct SwitchArgs {
    /// topic id（完整 uuid 或 8 字符前缀）或 title 字面量。
    pub id_or_title: String,
}

#[derive(Debug, Args)]
pub struct ArchiveArgs {
    /// 同上，topic id 或 title。
    pub id_or_title: String,
}

pub async fn run(args: TopicArgs) -> Result<()> {
    match args.cmd {
        TopicCmd::New(a) => run_new(a).await,
        TopicCmd::List(a) => run_list(a).await,
        TopicCmd::Switch(a) => run_switch(a).await,
        TopicCmd::Archive(a) => run_archive(a).await,
    }
}

async fn open_store() -> Result<TopicStore> {
    let path = im_db::default_db_path().context("$HOME 未设——无法定位 ~/.fuxi/im.db")?;
    let pool = im_db::init_at(&path)
        .await
        .with_context(|| format!("打开 {} 失败", path.display()))?;
    Ok(TopicStore::new(pool))
}

async fn run_new(args: NewArgs) -> Result<()> {
    let title = args.title.trim();
    if title.is_empty() {
        bail!("title 不能为空");
    }
    let store = open_store().await?;
    let meta = store.create(title).await.context("topic create")?;
    println!("{}", meta.id.0);
    eprintln!("✔ 新 topic: {} · {}", meta.id, meta.title);
    eprintln!("  切到此 topic：fuxi topic switch {}", meta.id.0);
    Ok(())
}

async fn run_list(args: ListArgs) -> Result<()> {
    let store = open_store().await?;
    let items = store
        .list(args.include_archived)
        .await
        .context("topic list")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if items.is_empty() {
        println!("(无 topic)");
        return Ok(());
    }
    // 文本表：id 短码 · title · last_active · 归档标记
    println!(
        "{:<10}  {:<40}  {:<20}  state",
        "id8", "title", "last_active"
    );
    for m in items {
        let id8 = &m.id.0.to_string()[..8];
        let last = m.last_active_at.format("%Y-%m-%d %H:%M:%S").to_string();
        let state = if m.is_archived() {
            "archived"
        } else {
            "active"
        };
        println!("{:<10}  {:<40}  {:<20}  {}", id8, m.title, last, state);
    }
    Ok(())
}

async fn run_switch(args: SwitchArgs) -> Result<()> {
    let store = open_store().await?;
    let topic_id = resolve_topic(&store, &args.id_or_title).await?;
    // daemon ipc：让 daemon 端 switch_topic_to 跑完整流程。
    let resp = crate::client::send(crate::ipc::Command::SwitchTopic {
        topic_id: topic_id.0.to_string(),
    })
    .await
    .context("发 SwitchTopic 给 daemon 失败——fuxi-im / fuxi up 没在跑？")?;
    match resp {
        crate::ipc::Response::Ok { data } => {
            println!("✔ 已切到 topic {topic_id}");
            if let Some(v) = data.get("topic_id") {
                eprintln!("  topic_id={v}");
            }
            Ok(())
        }
        crate::ipc::Response::Err { error } => Err(anyhow!("daemon: {error}")),
        crate::ipc::Response::Pong => Err(anyhow!("意外的 Pong 响应")),
    }
}

async fn run_archive(args: ArchiveArgs) -> Result<()> {
    let store = open_store().await?;
    let topic_id = resolve_topic(&store, &args.id_or_title).await?;
    store.archive(topic_id).await.context("topic archive")?;
    println!("✔ 已归档 topic {topic_id}");
    Ok(())
}

/// 解析「id 或 title」到 TopicId：先按 uuid 试；不是 uuid 则当 title 在 topics
/// 表里反查；前缀（8 字符）暂未支持（实现成本 vs 实际收益不划算——玄女总是
/// 用全 uuid 调，人类用 title 调）。
async fn resolve_topic(store: &TopicStore, input: &str) -> Result<TopicId> {
    let s = input.trim();
    if let Ok(uuid) = Uuid::parse_str(s) {
        return Ok(TopicId::from(uuid));
    }
    // 按 title 在 active+archived 范围内找
    let all = store.list(true).await.context("topic list for lookup")?;
    let matches: Vec<_> = all.iter().filter(|t| t.title == s).collect();
    match matches.len() {
        0 => bail!("没找到 id 或 title 匹配 {s:?} 的 topic"),
        1 => Ok(matches[0].id),
        n => bail!("title {s:?} 有 {n} 个同名 topic，请用 uuid 区分"),
    }
}
