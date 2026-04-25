//! P5 baseline bench 的共享 helper——β throughput / γ latency 的 bench 文件
//! 都通过 `#[path = "dist_bench_common.rs"] mod common;` 来 include 此文件。
//!
//! 实际的脚手架（StubAdapter / BenchHarness / write_baseline_report）住在
//! `fuxi_cli::bench_support`——本文件只是 re-export 让 bench 调用更短。
//! 不是独立 bench target（Cargo.toml 关了 autobench；[[bench]] 显式枚举）。

#![allow(dead_code, unused_imports)]

pub use fuxi_cli::bench_support::{
    BenchHarness, BenchSection, spawn_controller_with_workers, stub_adapter_with_sleep,
    write_baseline_report,
};

/// 报告输出路径——`docs/benchmarks/baseline-<UTC date>.md`。聚合 bench 主入
/// 口先调一次定下文件名，再把多 section 累积写入。
pub fn baseline_report_path() -> std::path::PathBuf {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    // 走 manifest 相对路径——cargo bench 的 cwd 是 crate 根。
    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_root
        .join("../../docs/benchmarks")
        .join(format!("baseline-{date}.md"))
}
