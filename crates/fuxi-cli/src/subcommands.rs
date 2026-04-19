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
}

pub async fn run_spawn(args: SpawnArgs) -> Result<()> {
    let resp = client::send(Command::Spawn {
        role: args.role,
        name: args.name,
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
