//! 伏羲 CLI 入口。
//!
//! 占位符：当前只能打印 "伏羲待命"。下面子任务会填 `fuxi up` / `fuxi spawn`
//! / `fuxi watch` 等子命令。

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "fuxi", version, about = "伏羲·玄女门客军团的指挥台", long_about = None)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _ = Cli::parse();
    tracing_subscriber::fmt().with_env_filter("info").init();
    tracing::info!("伏羲待命中。这里之后会长出真正的指挥台。");
    Ok(())
}
