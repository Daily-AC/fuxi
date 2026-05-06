//! `fuxi bug report` —— 玄女自报 bug / 改进建议入口。
//!
//! 落 `~/.fuxi/im.db` 的 notifications 表（kind="bug"），PWA「通知」tab 立即可见。
//!
//! 设计：CLI 直开 SQLite 而不是走 HTTP。原因：
//! - cc subprocess 跟 PWA cookie auth 不通，走 HTTP 要单独换一套 token
//! - SQLite WAL 模式多写并发安全，IM-write + CLI-write 不冲突
//! - 跟 `fuxi memory record` / `fuxi profile set` 同路径——玄女工具直开 DB
//!
//! 玄女何时跑这个：撞到 fuxi 平台本身的 bug / 不爽 / 改进建议。**不是**业务
//! task 失败的报告（那个走 task lifecycle）。spawn 时 system prompt addendum
//! 教过她。

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use fuxi_im::db as im_db;
use fuxi_im::notifications::{NewNotification, NotificationStore};

#[derive(Debug, Args)]
pub struct BugArgs {
    #[command(subcommand)]
    pub cmd: BugCmd,
}

#[derive(Debug, Subcommand)]
pub enum BugCmd {
    /// 上报一条 bug / 改进建议。
    /// `fuxi bug report --title "..." --body "..." [--severity bug|warn|wish] [--task <id>]`
    Report(ReportArgs),
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// 短标题（PWA 通知列表显示）。
    #[arg(long)]
    pub title: String,
    /// 详细描述（多行/代码片段都行）。
    #[arg(long, default_value_t = String::new())]
    pub body: String,
    /// 严重度：`bug`（影响功能）/ `warn`（不影响但烦）/ `wish`（功能改进建议）。
    /// 映射到 notifications.severity（`error` / `warn` / `info`）。
    #[arg(long, default_value = "bug")]
    pub severity: String,
    /// 可选 task_id 关联——撞 bug 时如果在跑某 task 顺手填上，PWA 能跳转。
    #[arg(long)]
    pub task: Option<String>,
    /// 可选 agent_id 关联——同上，谁踩到的 bug。
    #[arg(long)]
    pub agent: Option<String>,
}

pub async fn run(args: BugArgs) -> Result<()> {
    match args.cmd {
        BugCmd::Report(a) => run_report(a).await,
    }
}

async fn run_report(args: ReportArgs) -> Result<()> {
    if args.title.trim().is_empty() {
        anyhow::bail!("--title 不能为空");
    }
    let path = im_db::default_db_path().context("$HOME 未设置——无法定位 ~/.fuxi/im.db")?;
    let pool = im_db::init_at(&path)
        .await
        .with_context(|| format!("打开 {} 失败", path.display()))?;
    let store = NotificationStore::new(pool);

    // severity 短名映射到表里的标准值。bug = error（红），warn = warn（黄），wish = info（蓝）。
    // 玄女 prompt 里只暴露三档关键词，DB 内用统一 schema 跟其他 kind 对齐。
    let severity = match args.severity.trim() {
        "bug" => "error",
        "warn" => "warn",
        "wish" => "info",
        other => {
            anyhow::bail!("--severity 只接受 bug|warn|wish，得到 {other}");
        }
    };

    let n = NewNotification {
        kind: "bug".into(),
        severity: severity.into(),
        title: args.title.trim().to_string(),
        body: args.body,
        task_id: args.task,
        agent_id: args.agent,
        metadata: None,
    };
    let saved = store.insert(n).await.context("写 notifications 表失败")?;

    // stdout 给玄女看的 ack——id 让她记下，必要时之后能 close
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "id": saved.id,
            "created_at": saved.created_at,
        })
    );
    Ok(())
}
