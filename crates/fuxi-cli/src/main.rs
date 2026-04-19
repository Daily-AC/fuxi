//! `fuxi` 二进制入口。
//!
//! v0.1 子命令：
//! - `fuxi`（无参）—— 进入 REPL，用户直接和玄女对话（**用户视角的入口**）
//! - `fuxi demo` —— 端到端最小演示（P1 遗产，验证 cc 链路）
//! - `fuxi up` —— 平台长跑：EventBus + Firehose Hub + **daemon Unix socket**
//! - `fuxi watch` —— 连 Hub 打开 TUI 观察器
//! - `fuxi spawn/dispatch/intervene/status/list/kill` —— **玄女的工具子命令**
//!   （玄女的 CC 实例通过 Bash 调它们，人类一般不直接用）
//!
//! 用户视角铁律（见 `docs/superpowers/specs/2026-04-19-v0.1-scenario.md §1`）：
//! **用户只跟玄女对话**。这些子命令对玄女可见、对用户不可见。

use clap::{Parser, Subcommand};

mod client;
mod daemon;
mod demo;
mod ipc;
mod memory_cmd;
mod repl;
mod skill;
mod subcommands;
mod up;
mod watch;

#[derive(Debug, Parser)]
#[command(name = "fuxi", version, about = "伏羲·玄女门客军团的指挥台", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 端到端演示：spawn 一个 cc 门客，执行 prompt，实时打印事件。
    Demo(demo::Args),
    /// 启动伏羲平台（EventBus + Firehose Hub + daemon）长跑。
    Up(up::Args),
    /// 连上运行中的 Hub，打开 TUI 观察器。
    Watch(watch::Args),
    /// 【玄女工具】起一个门客。
    Spawn(subcommands::SpawnArgs),
    /// 【玄女工具】把任务派给指定门客。
    Dispatch(subcommands::DispatchArgs),
    /// 【玄女工具】向门客发话（追加式 / 打断式）。
    Intervene(subcommands::InterveneArgs),
    /// 【玄女工具】查看门客状态。
    Status(subcommands::StatusArgs),
    /// 【玄女工具】列出所有门客。
    List(subcommands::ListArgs),
    /// 【玄女工具】关停指定门客。
    Kill(subcommands::KillArgs),
    /// 【玄女工具】请示用户前标记任务 Blocked。
    Block(subcommands::BlockArgs),
    /// 【玄女工具】用户授权通过后解锁任务。
    Resume(subcommands::ResumeArgs),
    /// 点将台管理：list / stage / approve / reject / activate。
    Skill(skill::SkillArgs),
    /// 【玄女工具】策府记忆存取（甲骨 + 河图洛书）。
    Memory(memory_cmd::MemoryArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.cmd {
        None => repl::run(Default::default()).await,
        Some(Command::Demo(args)) => demo::run(args).await,
        Some(Command::Up(args)) => up::run(args).await,
        Some(Command::Watch(args)) => watch::run(args).await,
        Some(Command::Spawn(args)) => subcommands::run_spawn(args).await,
        Some(Command::Dispatch(args)) => subcommands::run_dispatch(args).await,
        Some(Command::Intervene(args)) => subcommands::run_intervene(args).await,
        Some(Command::Status(args)) => subcommands::run_status(args).await,
        Some(Command::List(args)) => subcommands::run_list(args).await,
        Some(Command::Kill(args)) => subcommands::run_kill(args).await,
        Some(Command::Block(args)) => subcommands::run_block(args).await,
        Some(Command::Resume(args)) => subcommands::run_resume(args).await,
        Some(Command::Skill(args)) => skill::run(args).await,
        Some(Command::Memory(args)) => memory_cmd::run(args).await,
    }
}

/// 默认把日志写到 stderr，留 stdout 给 demo 的事件流输出。
/// `RUST_LOG` 可覆盖；未设时缺省 `info,fuxi=debug`。
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,fuxi=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
