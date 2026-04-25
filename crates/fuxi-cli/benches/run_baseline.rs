//! P5 baseline bench 主入口——聚合 β throughput + γ latency section，
//! 写到 `docs/benchmarks/baseline-<UTC date>.md`。
//!
//! 不用 criterion：spinup-N-worker × dispatch-M-job 是系统级 wall-clock 测，
//! criterion 的 µs-级 micro-benchmark 模型不合身。custom harness + markdown
//! 报告更直观，跑完直接进 docs/ 给人读。
//!
//! 跑法：`cargo bench -p fuxi-cli --bench run_baseline`

#[path = "dist_bench_common.rs"]
mod common;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    eprintln!("[baseline] 开跑 throughput section …");
    let throughput = fuxi_cli::bench_support::throughput::run().await;
    eprintln!("[baseline] 开跑 latency section …");
    let latency = fuxi_cli::bench_support::latency::run().await;

    let path = common::baseline_report_path();
    let sections = vec![throughput, latency];
    common::write_baseline_report(&path, &sections).expect("写 baseline 报告失败");
    eprintln!("[baseline] 报告写入 {}", path.display());
}
