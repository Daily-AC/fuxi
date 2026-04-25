//! β path 占位：throughput bench section。任务 #2 owner 替换 `run()` 内容。
//!
//! 契约：返回一份 `BenchSection`（headers + rows + notes），
//! `bench_support::write_baseline_report` 会按 markdown 表格渲染。

use super::BenchSection;

/// 主入口——`benches/run_baseline.rs` 调本函数收集 throughput section。
pub async fn run() -> BenchSection {
    BenchSection {
        name: "Throughput · TBD (β path)".into(),
        headers: vec![
            "workers".into(),
            "concurrency".into(),
            "sleep_ms".into(),
            "tasks/sec".into(),
        ],
        rows: vec![],
        notes: "_(placeholder——任务 #2 β 实装后填实数)_".into(),
    }
}
