//! `fuxi issue` —— issue 工作流 CLI（v1-session19）。
//!
//! 玄女上报 → Claude 用这个子命令拉取/修复/关联 fix → 用户/玄女测试 → 关闭。
//! 直开 `~/.fuxi/im.db` 同 bug_cmd——CLI 跟 PWA 走不同 auth 但同一 SQLite，
//! WAL 模式多写并发安全。
//!
//! 子命令：
//! - `fuxi issue list [--status open|awaiting_test|closed|all] [--limit N] [--json]`
//! - `fuxi issue show <id>` —— body + fix_refs + events 时间线
//! - `fuxi issue close <id> [--note <text>] [--actor <s>]`
//! - `fuxi issue reopen <id> [--note <text>] [--actor <s>]`
//! - `fuxi issue link-fix <id> [--commit <sha>] [--branch <name>] [--summary <text>] [--actor <s>]`
//!   `--commit` 缺省自动 `git rev-parse HEAD`；`--branch` 缺省 `git symbolic-ref`；
//!   `--summary` 缺省取 `git log -1 --pretty=%s`。
//!
//! id 接受全 uuid 或 8 字符前缀（unique 时）——前缀用 SQL `LIKE` 找匹配。

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};
use fuxi_im::db as im_db;
use fuxi_im::notifications::{
    FixRef, IssueStatus, ListFilter, Notification, NotificationStore, now_iso,
};

#[derive(Debug, Args)]
pub struct IssueArgs {
    #[command(subcommand)]
    pub cmd: IssueCmd,
}

#[derive(Debug, Subcommand)]
pub enum IssueCmd {
    /// 列出 issue。默认显示 open + awaiting_test，--status 指定时只显示该态。
    List(ListArgs),
    /// 显示单条 issue 详情（body + fix_refs + events 时间线）。
    Show(ShowArgs),
    /// 关闭 issue（status='closed' + 写 events.closed）。
    Close(CloseArgs),
    /// 重开 issue（status='open' + 清 closed_at）。
    Reopen(CloseArgs),
    /// 关联 fix commit，自动转 status='awaiting_test'（原状态 open 时）。
    LinkFix(LinkFixArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// 过滤状态：open / awaiting_test / closed / all。默认 open+awaiting_test。
    #[arg(long, default_value = "active")]
    pub status: String,
    /// 上限，默认 50。
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
    /// 输出 JSON 数组（机器可读）。否则人读表格。
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// issue id（uuid 或 8 字符前缀）。
    pub id: String,
}

#[derive(Debug, Args)]
pub struct CloseArgs {
    pub id: String,
    /// 备注理由，写进 events.note。
    #[arg(long)]
    pub note: Option<String>,
    /// 标记是谁动作的，默认 "claude"（CLI 主要由 Claude 用）。
    #[arg(long, default_value = "claude")]
    pub actor: String,
}

#[derive(Debug, Args)]
pub struct LinkFixArgs {
    pub id: String,
    /// commit sha，缺省取 cwd 的 `git rev-parse HEAD`。
    #[arg(long)]
    pub commit: Option<String>,
    /// 分支名，缺省取 cwd 的 `git symbolic-ref --short HEAD`。
    #[arg(long)]
    pub branch: Option<String>,
    /// 一句话总结，缺省取 cwd 的 `git log -1 --pretty=%s`。
    #[arg(long)]
    pub summary: Option<String>,
    /// 标记是谁挂的 fix，默认 "claude"。
    #[arg(long, default_value = "claude")]
    pub actor: String,
}

pub async fn run(args: IssueArgs) -> Result<()> {
    let store = open_store().await?;
    match args.cmd {
        IssueCmd::List(a) => run_list(&store, a).await,
        IssueCmd::Show(a) => run_show(&store, a).await,
        IssueCmd::Close(a) => run_close(&store, a, IssueStatus::Closed).await,
        IssueCmd::Reopen(a) => run_close(&store, a, IssueStatus::Open).await,
        IssueCmd::LinkFix(a) => run_link_fix(&store, a).await,
    }
}

async fn open_store() -> Result<NotificationStore> {
    let path = im_db::default_db_path().context("$HOME 未设置——无法定位 ~/.fuxi/im.db")?;
    let pool = im_db::init_at(&path)
        .await
        .with_context(|| format!("打开 {} 失败", path.display()))?;
    Ok(NotificationStore::new(pool))
}

async fn run_list(store: &NotificationStore, args: ListArgs) -> Result<()> {
    let (filter_status, include_closed) = match args.status.trim() {
        "open" => (Some(IssueStatus::Open), false),
        "awaiting_test" => (Some(IssueStatus::AwaitingTest), false),
        "closed" => (Some(IssueStatus::Closed), true),
        "all" => (None, true),
        // "active" = 默认两态合集 (open + awaiting_test)，靠 include_closed=false 过滤
        "active" | "" => (None, false),
        other => bail!("--status 可选 open / awaiting_test / closed / active / all；得到 {other}"),
    };
    let issues = store
        .list(ListFilter {
            kind: Some("bug".into()),
            status: filter_status,
            include_closed,
            limit: Some(args.limit),
        })
        .await
        .context("拉 issue list 失败")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&issues).context("issue list json")?
        );
        return Ok(());
    }

    if issues.is_empty() {
        println!("(没有匹配的 issue)");
        return Ok(());
    }

    // 表格形式：id8 status severity title 截断 created_at
    println!(
        "{:<10}  {:<15}  {:<8}  {:<60}  CREATED",
        "ID", "STATUS", "SEV", "TITLE"
    );
    println!("{}", "─".repeat(115));
    for n in issues {
        let id8: String = n.id.chars().take(8).collect();
        let title = if n.title.chars().count() > 58 {
            let truncated: String = n.title.chars().take(57).collect();
            format!("{truncated}…")
        } else {
            n.title.clone()
        };
        let created = n.created_at.split('T').next().unwrap_or(&n.created_at);
        println!(
            "{:<10}  {:<15}  {:<8}  {:<60}  {}",
            id8, n.status, n.severity, title, created
        );
    }
    Ok(())
}

async fn run_show(store: &NotificationStore, args: ShowArgs) -> Result<()> {
    let n = resolve_issue(store, &args.id).await?;
    println!("# {}  ({})", n.title, n.status);
    println!("id        : {}", n.id);
    println!("kind      : {}", n.kind);
    println!("severity  : {}", n.severity);
    println!("created_at: {}", n.created_at);
    if let Some(c) = &n.closed_at {
        println!("closed_at : {c}");
    }
    if let Some(t) = &n.task_id {
        println!("task_id   : {t}");
    }
    if let Some(a) = &n.agent_id {
        println!("agent_id  : {a}");
    }

    println!(
        "\n## body\n{}",
        if n.body.is_empty() { "(空)" } else { &n.body }
    );

    if !n.fix_refs.is_empty() {
        println!("\n## fix commits ({})", n.fix_refs.len());
        for f in &n.fix_refs {
            let branch = f.branch.as_deref().unwrap_or("-");
            let summary = f.summary.as_deref().unwrap_or("-");
            println!("  {} [{}]  {}  ({})", f.commit_sha, branch, summary, f.at);
        }
    }

    if !n.events.is_empty() {
        println!("\n## events ({})", n.events.len());
        for e in &n.events {
            let from = e.from.as_deref().unwrap_or("-");
            let to = e.to.as_deref().unwrap_or("-");
            let note = e
                .note
                .as_deref()
                .map(|s| format!("  // {s}"))
                .unwrap_or_default();
            println!(
                "  {}  [{}] {}  {} → {}{}",
                e.at, e.actor, e.action, from, to, note
            );
        }
    }
    Ok(())
}

async fn run_close(store: &NotificationStore, args: CloseArgs, target: IssueStatus) -> Result<()> {
    let n = resolve_issue(store, &args.id).await?;
    match target {
        IssueStatus::Closed => store.close(&n.id, &args.actor, args.note.as_deref()).await,
        IssueStatus::Open => store.reopen(&n.id, &args.actor, args.note.as_deref()).await,
        IssueStatus::AwaitingTest => store
            .update_status(
                &n.id,
                IssueStatus::AwaitingTest,
                &args.actor,
                args.note.as_deref(),
            )
            .await
            .map(|_| ()),
    }
    .with_context(|| format!("更新 issue {} 状态失败", n.id))?;
    println!(
        "{}",
        serde_json::json!({"ok": true, "id": n.id, "status": target.as_str()})
    );
    Ok(())
}

async fn run_link_fix(store: &NotificationStore, args: LinkFixArgs) -> Result<()> {
    let n = resolve_issue(store, &args.id).await?;
    let commit = match args.commit {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => git_head_sha().context("自动取 HEAD sha 失败——传 --commit 显式指定")?,
    };
    let branch = match args.branch {
        Some(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => git_current_branch().ok(),
    };
    let summary = match args.summary {
        Some(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => git_head_summary().ok(),
    };
    let fix = FixRef {
        commit_sha: commit.clone(),
        branch,
        summary,
        at: now_iso(),
    };
    store
        .link_fix(&n.id, fix, &args.actor)
        .await
        .with_context(|| format!("link-fix issue {}", n.id))?;
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "id": n.id,
            "commit": commit,
            "status": "awaiting_test"
        })
    );
    Ok(())
}

/// 接受全 uuid 或前缀（>=4 字符）。前缀模糊匹配多条则返冲突报错。
async fn resolve_issue(store: &NotificationStore, id_or_prefix: &str) -> Result<Notification> {
    let trimmed = id_or_prefix.trim();
    if trimmed.is_empty() {
        bail!("id 不能为空");
    }
    if let Some(n) = store.get(trimmed).await.context("get issue")? {
        return Ok(n);
    }
    if trimmed.len() < 4 {
        bail!("id 前缀至少 4 字符（避免误匹配）；得到 {trimmed}");
    }
    // 前缀匹配——拿全部 issue（含 closed）按 id startswith 匹配
    let all = store
        .list(ListFilter {
            include_closed: true,
            limit: Some(500),
            ..Default::default()
        })
        .await
        .context("list for prefix match")?;
    let matches: Vec<Notification> = all
        .into_iter()
        .filter(|n| n.id.starts_with(trimmed))
        .collect();
    match matches.len() {
        0 => Err(anyhow!("找不到 issue {trimmed}")),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => Err(anyhow!(
            "id 前缀 {trimmed} 匹配 {n} 条——加更长前缀或用全 uuid"
        )),
    }
}

fn git_head_sha() -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .context("调 git rev-parse")?;
    if !out.status.success() {
        bail!(
            "git rev-parse 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_current_branch() -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .context("git symbolic-ref")?;
    if !out.status.success() {
        bail!("git symbolic-ref 失败（可能 detached HEAD）");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_head_summary() -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=%s", "HEAD"])
        .output()
        .context("git log summary")?;
    if !out.status.success() {
        bail!("git log 失败");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
