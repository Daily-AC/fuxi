//! γ path 占位：latency bench section。任务 #3 owner 替换 `run()` 内容。
//!
//! 契约：返回一份 `BenchSection`（headers + rows + notes），
//! `bench_support::write_baseline_report` 会按 markdown 表格渲染。

use super::BenchSection;

/// 主入口——`benches/run_baseline.rs` 调本函数收集 latency section。
pub async fn run() -> BenchSection {
    BenchSection {
        name: "Latency · TBD (γ path)".into(),
        headers: vec![
            "metric".into(),
            "p50".into(),
            "p99".into(),
            "samples".into(),
        ],
        rows: vec![],
        notes: "_(placeholder——任务 #3 γ 实装后填实数)_".into(),
    }
}
