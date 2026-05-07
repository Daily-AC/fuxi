//! TB.2 poll_ms 参数消融——固定 `4w × 10ms × 400 tasks`，扫 poll_ms ∈
//! {5, 10, 25, 50, 100}。
//!
//! 跑法：`cargo bench -p fuxi-cli --bench poll_scan`
//!
//! 不进 run_baseline 主报告——poll 只关 controller 编排链路而非系统全貌，
//! 论文用单独 section 展示折线/拐点更直观。

#[path = "dist_bench_common.rs"]
mod common;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    eprintln!("[poll_scan] 开跑 …");
    let section = fuxi_cli::bench_support::throughput::poll_scan().await;

    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = crate_root.join("../../deliverables/thesis-v2/benchmarks/poll-scan/results.md");
    common::write_baseline_report(&path, &[section]).expect("写 poll_scan 报告失败");
    eprintln!("[poll_scan] 报告写入 {}", path.display());
}
