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

mod click_registry;
mod client;
mod daemon;
mod demo;
mod ipc;
mod memory_cmd;
mod repl;
mod session;
mod skill;
mod subcommands;
mod theme;
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
    /// 【玄女工具】更漏——trigger 管理（cron / once / fs / webhook）。
    #[command(subcommand)]
    Cron(CronCmd),
}

#[derive(Debug, Subcommand)]
enum CronCmd {
    /// 登记 cron trigger。
    Add(subcommands::CronAddArgs),
    /// 登记一次性 trigger。
    Once(subcommands::CronOnceArgs),
    /// 登记 fs_watch trigger。
    Watch(subcommands::CronWatchArgs),
    /// 登记 webhook trigger。
    Webhook(subcommands::CronWebhookArgs),
    /// 列所有 triggers。
    List(subcommands::CronListArgs),
    /// 手动 fire。
    Fire(subcommands::CronFireArgs),
    /// 删 trigger。
    Remove(subcommands::CronRemoveArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // TUI 模式必须**在 init_tracing 之前**把 stderr 重定向到文件——否则
    // fuxi 启动期（hub/daemon/spawn 玄女）的 tracing 会污染用户 shell，
    // 退 TUI 后 alt screen 撤掉时一把全冒出来。踩过，见 docs/session-review-2026-04-20.md。
    if is_tui_mode(&cli.cmd)
        && let Err(e) = redirect_stderr_to_log("/tmp/fuxi.log")
    {
        eprintln!("⚠ 无法把 stderr 重定向到 /tmp/fuxi.log: {e}（TUI 下日志可能污染画面）");
    }

    init_tracing();
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
        Some(Command::Cron(c)) => match c {
            CronCmd::Add(args) => subcommands::run_cron_add(args).await,
            CronCmd::Once(args) => subcommands::run_cron_once(args).await,
            CronCmd::Watch(args) => subcommands::run_cron_watch(args).await,
            CronCmd::Webhook(args) => subcommands::run_cron_webhook(args).await,
            CronCmd::List(args) => subcommands::run_cron_list(args).await,
            CronCmd::Fire(args) => subcommands::run_cron_fire(args).await,
            CronCmd::Remove(args) => subcommands::run_cron_remove(args).await,
        },
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

/// 是否是进 alt-screen TUI 的子命令。
fn is_tui_mode(cmd: &Option<Command>) -> bool {
    match cmd {
        None => true, // 无参 → repl
        Some(Command::Watch(_)) => true,
        Some(Command::Demo(a)) if a.tui => true,
        _ => false,
    }
}

/// 把进程的 stderr（fd 2）重定向到日志文件。Unix `dup2(2)`.
///
/// **时机**：**必须** init_tracing 之前调——tracing subscriber 用 `std::io::stderr()`
/// 底层写 fd 2。dup2 之后 fd 2 指向文件，后续所有 stderr 写入都进文件。
#[cfg(unix)]
fn redirect_stderr_to_log(path: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)?;
    // SAFETY: dup2 对 valid fd 是安全调用；file 在作用域内有效。
    let ret = unsafe { libc_dup2(file.as_raw_fd(), 2) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn redirect_stderr_to_log(_path: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("stderr redirect only on unix"))
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "dup2"]
    fn libc_dup2(oldfd: i32, newfd: i32) -> i32;
}
