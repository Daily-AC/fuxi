//! 玄女的工具子命令——`fuxi spawn/dispatch/intervene/status/list/kill`。
//!
//! 每个都是 daemon client 的薄壳。玄女 CC 实例通过 Bash 调用这些；
//! 人类直接调也 OK（调试、救急）。
//!
//! 设计公约：
//! - 成功：stdout 输出一行可读的 JSON 摘要（方便玄女 parse）
//! - 失败：stderr 打错误，exit code 非零
//! - **用户视角不应该进这些子命令**——这是写在 v0.1 scenario spec §1 铁律里
//!   的。这些是玄女的 Bash 工具而已。

use crate::client;
use crate::ipc::{Command, InterveneMode, Response};
use anyhow::{Result, anyhow};
use clap::Args as ClapArgs;

// ── spawn ──

#[derive(Debug, ClapArgs)]
pub struct SpawnArgs {
    /// 门客角色。必须存在 `skills/<role>/SKILL.md`。
    #[arg(long)]
    pub role: String,
    /// 可选名字（默认 role-N）。
    #[arg(long)]
    pub name: Option<String>,
    /// P2 召回：续写指定 task 的 cc session（与 --recall-role 互斥）。
    /// 接受 `task-<uuid>` 或裸 `<uuid>`——daemon 端会标准化成 `task-<uuid>`。
    #[arg(long = "recall-task", conflicts_with = "recall_role")]
    pub recall_task: Option<String>,
    /// P2 召回：续写该 role 最近活动的 session（subject=`role-<role>`）。
    #[arg(long = "recall-role")]
    pub recall_role: Option<String>,
}

pub async fn run_spawn(args: SpawnArgs) -> Result<()> {
    let resp = client::send(Command::Spawn {
        role: args.role,
        name: args.name,
        recall_task: args.recall_task,
        recall_role: args.recall_role,
    })
    .await?;
    print_response(resp)
}

// ── dispatch ──

#[derive(Debug, ClapArgs)]
pub struct DispatchArgs {
    /// 目标门客 id（UUID）。
    #[arg(long = "to")]
    pub agent_id: String,
    /// 任务标题。
    #[arg(long, default_value = "ad-hoc")]
    pub title: String,
    /// 任务正文（prompt）。位置参数——剩余参数拼起来。
    #[arg(trailing_var_arg = true, required = true)]
    pub body: Vec<String>,
}

pub async fn run_dispatch(args: DispatchArgs) -> Result<()> {
    let body = args.body.join(" ");
    let resp = client::send(Command::Dispatch {
        agent_id: args.agent_id,
        title: args.title,
        body: if body.is_empty() { None } else { Some(body) },
    })
    .await?;
    print_response(resp)
}

// ── intervene ──

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum InterveneModeArg {
    Append,
    Interrupt,
}

impl From<InterveneModeArg> for InterveneMode {
    fn from(a: InterveneModeArg) -> Self {
        match a {
            InterveneModeArg::Append => InterveneMode::Append,
            InterveneModeArg::Interrupt => InterveneMode::Interrupt,
        }
    }
}

#[derive(Debug, ClapArgs)]
pub struct InterveneArgs {
    /// 目标门客 id。
    #[arg(long = "to")]
    pub agent_id: String,
    /// 介入模式：append（下 turn 看到）/ interrupt（打断当前 turn）。
    #[arg(long, default_value = "append")]
    pub mode: InterveneModeArg,
    /// 介入文本。
    #[arg(trailing_var_arg = true, required = true)]
    pub text: Vec<String>,
}

pub async fn run_intervene(args: InterveneArgs) -> Result<()> {
    let text = args.text.join(" ");
    let resp = client::send(Command::Intervene {
        agent_id: args.agent_id,
        mode: args.mode.into(),
        text,
    })
    .await?;
    print_response(resp)
}

// ── status ──

#[derive(Debug, ClapArgs)]
pub struct StatusArgs {
    /// 查指定 id；省略则返回全局概览。
    #[arg(long)]
    pub id: Option<String>,
}

pub async fn run_status(args: StatusArgs) -> Result<()> {
    let resp = client::send(Command::Status { agent_id: args.id }).await?;
    print_response(resp)
}

// ── list ──

#[derive(Debug, ClapArgs)]
pub struct ListArgs {}

pub async fn run_list(_args: ListArgs) -> Result<()> {
    let resp = client::send(Command::List).await?;
    print_response(resp)
}

// ── block ──

#[derive(Debug, ClapArgs)]
pub struct BlockArgs {
    /// 要 block 的 task id。
    #[arg(long)]
    pub task: String,
    /// 原因（例如 "awaiting_commit_approval"）。
    #[arg(long, default_value = "awaiting_user_approval")]
    pub reason: String,
}

pub async fn run_block(args: BlockArgs) -> Result<()> {
    let resp = client::send(Command::BlockTask {
        task_id: args.task,
        reason: args.reason,
    })
    .await?;
    print_response(resp)
}

// ── resume ──

#[derive(Debug, ClapArgs)]
pub struct ResumeArgs {
    /// 要 resume 的 task id。
    #[arg(long)]
    pub task: String,
    /// 用户的授权话（"同意" / "同意，但改 X"）。留空也行。
    #[arg(long)]
    pub input: Option<String>,
}

pub async fn run_resume(args: ResumeArgs) -> Result<()> {
    let resp = client::send(Command::ResumeTask {
        task_id: args.task,
        input: args.input,
    })
    .await?;
    print_response(resp)
}

// ── kill ──

#[derive(Debug, ClapArgs)]
pub struct KillArgs {
    #[arg(long = "id")]
    pub agent_id: String,
}

pub async fn run_kill(args: KillArgs) -> Result<()> {
    let resp = client::send(Command::Kill {
        agent_id: args.agent_id,
    })
    .await?;
    print_response(resp)
}

// ── 更漏（cron / triggers） ──

#[derive(Debug, ClapArgs)]
pub struct CronAddArgs {
    /// cron 表达式。支持 5 字段（分时日月周）或 6 字段（含秒）。
    pub expr: String,
    /// 触发意图（自然语言，玄女收到后读）。
    #[arg(long)]
    pub intent: String,
    /// 时区（例 `Asia/Shanghai`）。默认 UTC。
    #[arg(long)]
    pub tz: Option<String>,
    /// 绑定的玄女 session_id；省略 = 触发时新起一个 session。
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_add(args: CronAddArgs) -> Result<()> {
    let resp = client::send(Command::CronAdd {
        expr: args.expr,
        intent: args.intent,
        tz: args.tz,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronOnceArgs {
    /// 绝对时间（RFC3339，例 `2026-04-19T21:00:00+08:00`）。
    pub at: String,
    #[arg(long)]
    pub intent: String,
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_once(args: CronOnceArgs) -> Result<()> {
    let resp = client::send(Command::CronOnce {
        at: args.at,
        intent: args.intent,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronWatchArgs {
    /// 要监视的目录或文件路径。
    pub path: String,
    #[arg(long)]
    pub intent: String,
    /// 只关心的事件类型白名单（`create`/`modify`/`remove`/`access`）；空 = 全部。
    #[arg(long, value_delimiter = ',')]
    pub events: Vec<String>,
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_watch(args: CronWatchArgs) -> Result<()> {
    let resp = client::send(Command::CronWatch {
        path: args.path,
        intent: args.intent,
        events: args.events,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronWebhookArgs {
    #[arg(long)]
    pub intent: String,
    /// 可选共享密钥——命中时要求 header `x-fuxi-secret` 或 body `secret` 匹配。
    #[arg(long)]
    pub secret: Option<String>,
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_webhook(args: CronWebhookArgs) -> Result<()> {
    let resp = client::send(Command::CronWebhook {
        intent: args.intent,
        secret: args.secret,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronListArgs {}

pub async fn run_cron_list(_args: CronListArgs) -> Result<()> {
    let resp = client::send(Command::CronList).await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronFireArgs {
    /// trigger_id（例 `trg_<uuid>`）。
    pub id: String,
}

pub async fn run_cron_fire(args: CronFireArgs) -> Result<()> {
    let resp = client::send(Command::CronFire { id: args.id }).await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronRemoveArgs {
    pub id: String,
}

pub async fn run_cron_remove(args: CronRemoveArgs) -> Result<()> {
    let resp = client::send(Command::CronRemove { id: args.id }).await?;
    print_response(resp)
}

// ── 共用渲染 ──

/// 把 Response 打印到 stdout；Err 转成 anyhow 让 exit code 非零。
fn print_response(resp: Response) -> Result<()> {
    match resp {
        Response::Ok { data } => {
            println!("{}", data);
            Ok(())
        }
        Response::Pong => {
            println!("{{\"status\":\"pong\"}}");
            Ok(())
        }
        Response::Err { error } => Err(anyhow!(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--recall-task` 与 `--recall-role` 是 P2 召回的两条互斥入口；clap 的
    /// `conflicts_with` 必须在 wire 上即生效，避免错误同时给两个 flag 后
    /// 跑到 daemon 才报错——CLI 层早死早超生。
    #[test]
    fn spawn_args_recall_task_and_role_are_mutually_exclusive() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: SpawnArgs,
        }
        let r = W::try_parse_from([
            "w",
            "--role",
            "dev",
            "--recall-task",
            "t1",
            "--recall-role",
            "dev",
        ]);
        assert!(r.is_err(), "互斥 flag 同时给应失败");
    }

    /// 单独给 `--recall-task` 应正常解析；recall_role 必须留 None。
    #[test]
    fn spawn_args_recall_task_only() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: SpawnArgs,
        }
        let w = W::try_parse_from(["w", "--role", "dev", "--recall-task", "task-abc"]).unwrap();
        assert_eq!(w.a.recall_task.as_deref(), Some("task-abc"));
        assert!(w.a.recall_role.is_none());
    }
}
