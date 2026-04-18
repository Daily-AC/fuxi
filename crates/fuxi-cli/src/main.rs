//! `fuxi` 二进制入口。
//!
//! P1 三个子命令：
//! - `fuxi demo` —— 端到端最小演示：本地起 EventBus、spawn 一个 cc 门客、
//!   把它的事件流同时推进 bus + 打印到 stdout（或 TUI）。
//! - `fuxi up` —— 长跑平台进程：EventBus + Firehose Hub 对外暴露 WS/SSE/REST。
//! - `fuxi watch` —— 连到一个运行中的 Hub，用 ratatui TUI 观察事件。

use clap::{Parser, Subcommand};

mod demo;
mod up;
mod watch;

#[derive(Debug, Parser)]
#[command(name = "fuxi", version, about = "伏羲·玄女门客军团的指挥台", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 端到端演示：spawn 一个 cc 门客，执行 prompt，实时打印事件。
    Demo(demo::Args),
    /// 启动伏羲平台（EventBus + Firehose Hub）长跑。
    Up(up::Args),
    /// 连上运行中的 Hub，打开 TUI 观察器。
    Watch(watch::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.cmd {
        Command::Demo(args) => demo::run(args).await,
        Command::Up(args) => up::run(args).await,
        Command::Watch(args) => watch::run(args).await,
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
