//! TB.3 scalability——固定 `sleep_ms=10, poll_ms=5`，扫 worker_n ∈
//! {1, 2, 4, 8, 16}，每 worker 100 个 task。
//!
//! 跑法：`cargo bench -p fuxi-cli --bench scalability`
//!
//! 不进 run_baseline 主报告——scalability 是单独章节实验，独立 markdown
//! 让 Track C 画图脚本直接吃。

#[path = "dist_bench_common.rs"]
mod common;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    eprintln!("[scalability] 开跑 …");
    let section = fuxi_cli::bench_support::throughput::scalability_scan().await;

    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = crate_root.join("../../deliverables/thesis-v2/benchmarks/scalability/results.md");
    common::write_baseline_report(&path, &[section]).expect("写 scalability 报告失败");
    eprintln!("[scalability] 报告写入 {}", path.display());
}
