//! β path：throughput bench section。3×3 矩阵，每 cell 重复 5 次取**中位数**
//! （不取平均：GC pause / 调度抖动等长尾会把平均拉偏，中位数对异常更鲁棒）。
//!
//! 测的是**fuxi 跨节点 dispatch 编排开销**——刻意让每 worker `max_concurrency=1`
//! 把 per-worker 并发隔离掉，让 throughput 数字只反映"controller pull/report
//! + axum HTTP + tokio 调度"这条链路，不掺 worker 内并发因素。
//!
//! `理论上限_tps = N / sleep_sec`：N 个 worker 串行各跑一个 sleep_sec 的 job，
//! 每秒最多产 N/sleep_sec 个完成。fuxi 损耗 % = 1 - 实测/理论，越小越好。

use super::{BenchSection, spawn_controller_with_workers_tuned};
use std::time::{Duration, Instant};

/// poll_ms=5 的 reasoning 见 BenchSection.notes：默认 50ms 在 10ms sleep job
/// 场景下 idle wait 主导 wall_time，会把 fuxi 损耗数字虚高（测的是 idle 拨号
/// 不是 fuxi infra）。tuned 5ms 把这部分摊薄。
const TUNED_POLL_MS: u64 = 5;

/// 每个 cell 重复 5 次取中位数——3 是最少能形成"中位"的样本数；论文实验
/// （thesis-v2 TB.1）升级到 5 是为了把 GC pause / 调度抖动这类长尾的影响
/// 进一步压低，median 更稳。代价是 wall clock 比 3-run 长 ~1.7×。
const REPEATS: usize = 5;

#[derive(Clone, Copy)]
struct CellSpec {
    workers: usize,
    sleep_ms: u64,
    tasks: usize,
}

const MATRIX: &[CellSpec] = &[
    // 1 worker
    CellSpec {
        workers: 1,
        sleep_ms: 10,
        tasks: 100,
    },
    CellSpec {
        workers: 1,
        sleep_ms: 100,
        tasks: 50,
    },
    CellSpec {
        workers: 1,
        sleep_ms: 1000,
        tasks: 10,
    },
    // 4 workers
    CellSpec {
        workers: 4,
        sleep_ms: 10,
        tasks: 400,
    },
    CellSpec {
        workers: 4,
        sleep_ms: 100,
        tasks: 200,
    },
    CellSpec {
        workers: 4,
        sleep_ms: 1000,
        tasks: 40,
    },
    // 8 workers
    CellSpec {
        workers: 8,
        sleep_ms: 10,
        tasks: 800,
    },
    CellSpec {
        workers: 8,
        sleep_ms: 100,
        tasks: 400,
    },
    CellSpec {
        workers: 8,
        sleep_ms: 1000,
        tasks: 80,
    },
];

/// 跑 1 次 cell：spawn → enqueue → 等全部 done → 算 wall_ms。
async fn run_once(spec: CellSpec) -> u128 {
    // max_concurrency=1：见模块注释 reasoning。
    let harness =
        spawn_controller_with_workers_tuned(spec.workers, 1, spec.sleep_ms, TUNED_POLL_MS).await;

    // 所有 task done 的最长理论耗时 = tasks × sleep_ms（极端 1 worker 串行）；
    // 给 4× 容忍 + 5s base 处理 register/pull 启动期 + GC tail。
    let timeout_ms = (spec.tasks as u64 * spec.sleep_ms) * 4 + 5_000;
    let timeout = Duration::from_millis(timeout_ms);

    let job_ids = harness.enqueue_n(spec.tasks, "throughput").await;
    let t0 = Instant::now();
    harness.wait_until_done(&job_ids, timeout).await;
    let wall_ms = t0.elapsed().as_millis();
    harness.shutdown();
    // 给上一个 cell 的 axum/worker 完全释放端口和 task——避免下一 cell 抢资源。
    tokio::time::sleep(Duration::from_millis(50)).await;
    wall_ms
}

/// 跑 REPEATS 次取中位数 wall_ms。
async fn run_cell_median(spec: CellSpec) -> u128 {
    let mut runs = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        runs.push(run_once(spec).await);
    }
    runs.sort_unstable();
    runs[runs.len() / 2]
}

/// poll_ms=50 的 bonus 对比列——展示默认配置下 idle wait 怎么主导 10ms-job
/// 场景，证明 tuned config 不是装样子：选 1 worker × 3 sleep 档
/// （4/8 worker 行为与 1 worker 同向，加它们只是 wall clock 翻倍）。
const DEFAULT_POLL_MS: u64 = 50;
const BONUS_MATRIX: &[CellSpec] = &[
    CellSpec {
        workers: 1,
        sleep_ms: 10,
        tasks: 100,
    },
    CellSpec {
        workers: 1,
        sleep_ms: 100,
        tasks: 50,
    },
    CellSpec {
        workers: 1,
        sleep_ms: 1000,
        tasks: 10,
    },
];

async fn run_one_cell_with_poll(spec: CellSpec, poll_ms: u64) -> u128 {
    let harness =
        spawn_controller_with_workers_tuned(spec.workers, 1, spec.sleep_ms, poll_ms).await;
    let timeout_ms = (spec.tasks as u64 * spec.sleep_ms) * 4 + 5_000;
    let timeout = Duration::from_millis(timeout_ms);
    let job_ids = harness.enqueue_n(spec.tasks, "throughput-bonus").await;
    let t0 = Instant::now();
    harness.wait_until_done(&job_ids, timeout).await;
    let wall_ms = t0.elapsed().as_millis();
    harness.shutdown();
    tokio::time::sleep(Duration::from_millis(50)).await;
    wall_ms
}

async fn run_cell_median_with_poll(spec: CellSpec, poll_ms: u64) -> u128 {
    let mut runs = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        runs.push(run_one_cell_with_poll(spec, poll_ms).await);
    }
    runs.sort_unstable();
    runs[runs.len() / 2]
}

fn cell_row(spec: CellSpec, median_wall_ms: u128) -> Vec<String> {
    let wall_sec = median_wall_ms as f64 / 1000.0;
    let tps = if wall_sec > 0.0 {
        spec.tasks as f64 / wall_sec
    } else {
        f64::NAN
    };
    let theoretical_tps = spec.workers as f64 / (spec.sleep_ms as f64 / 1000.0);
    let overhead_pct = if theoretical_tps > 0.0 {
        (1.0 - tps / theoretical_tps) * 100.0
    } else {
        f64::NAN
    };
    eprintln!(
        "[throughput]   wall_ms={median_wall_ms} tps={tps:.2} 理论={theoretical_tps:.2} 损耗={overhead_pct:.1}%"
    );
    vec![
        spec.workers.to_string(),
        format!("{}ms", spec.sleep_ms),
        spec.tasks.to_string(),
        median_wall_ms.to_string(),
        format!("{tps:.2}"),
        format!("{theoretical_tps:.2}"),
        format!("{overhead_pct:.1}%"),
    ]
}

// ── TB.2 poll_ms 扫描（thesis-v2 实验 2）──────────────────────────────────
//
// 设计：固定 `worker_n=4, tasks_n=400, sleep_ms=10`，扫 poll_ms ∈
// {5, 10, 25, 50, 100}。这一组 cell 是 baseline matrix 里 `4w × 10ms × 400`
// 那一格的 poll_ms 切片——同 cell 但只调 poll，让"poll_ms 单独的影响"
// 一目了然，不被 worker_n / sleep_ms 维度交叉污染。

const POLL_SCAN_WORKERS: usize = 4;
const POLL_SCAN_SLEEP_MS: u64 = 10;
const POLL_SCAN_TASKS: usize = 400;
const POLL_SCAN_VALUES: &[u64] = &[5, 10, 25, 50, 100];

/// TB.2 入口——`benches/poll_scan.rs` 调本函数。
pub async fn poll_scan() -> BenchSection {
    let spec = CellSpec {
        workers: POLL_SCAN_WORKERS,
        sleep_ms: POLL_SCAN_SLEEP_MS,
        tasks: POLL_SCAN_TASKS,
    };
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(POLL_SCAN_VALUES.len());
    for &poll_ms in POLL_SCAN_VALUES {
        eprintln!(
            "[poll_scan] cell workers={} sleep_ms={} tasks={} poll_ms={poll_ms} ×{} repeats …",
            spec.workers, spec.sleep_ms, spec.tasks, REPEATS
        );
        let median = run_cell_median_with_poll(spec, poll_ms).await;
        rows.push(poll_scan_row(spec, poll_ms, median));
    }
    BenchSection {
        name: "poll_ms 扫描 · worker_n=4, tasks_n=400, sleep=10ms".into(),
        headers: vec![
            "poll_ms".into(),
            "median_wall_ms".into(),
            "tasks_per_sec".into(),
            "理论上限_tps".into(),
            "fuxi_损耗".into(),
        ],
        rows,
        notes: format!(
            "_{REPEATS}-run median · 固定 cell `{}w × {}ms × {} tasks`，仅扫 poll_ms。\
             理论 tps = worker_n / sleep_sec = {:.0}._",
            spec.workers,
            spec.sleep_ms,
            spec.tasks,
            spec.workers as f64 / (spec.sleep_ms as f64 / 1000.0)
        ),
    }
}

fn poll_scan_row(spec: CellSpec, poll_ms: u64, median_wall_ms: u128) -> Vec<String> {
    let wall_sec = median_wall_ms as f64 / 1000.0;
    let tps = if wall_sec > 0.0 {
        spec.tasks as f64 / wall_sec
    } else {
        f64::NAN
    };
    let theoretical_tps = spec.workers as f64 / (spec.sleep_ms as f64 / 1000.0);
    let overhead_pct = if theoretical_tps > 0.0 {
        (1.0 - tps / theoretical_tps) * 100.0
    } else {
        f64::NAN
    };
    eprintln!(
        "[poll_scan]   poll_ms={poll_ms} wall_ms={median_wall_ms} tps={tps:.2} 损耗={overhead_pct:.1}%"
    );
    vec![
        poll_ms.to_string(),
        median_wall_ms.to_string(),
        format!("{tps:.2}"),
        format!("{theoretical_tps:.2}"),
        format!("{overhead_pct:.1}%"),
    ]
}

// ── TB.3 scalability 扫描（thesis-v2 实验 3）──────────────────────────────
//
// 设计：固定 `sleep_ms=10, poll_ms=5`，扫 worker_n ∈ {1,2,4,8,16}，每
// worker 100 个 task（即 tasks_n = worker_n × 100）。observed tps vs 理论
// (worker_n × 100) 的比值 = scaling efficiency；理想线性时 = 100%。

const SCALABILITY_SLEEP_MS: u64 = 10;
const SCALABILITY_TASKS_PER_WORKER: usize = 100;
const SCALABILITY_WORKERS: &[usize] = &[1, 2, 4, 8, 16];

/// TB.3 入口——`benches/scalability.rs` 调本函数。
pub async fn scalability_scan() -> BenchSection {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(SCALABILITY_WORKERS.len());
    for &workers in SCALABILITY_WORKERS {
        let spec = CellSpec {
            workers,
            sleep_ms: SCALABILITY_SLEEP_MS,
            tasks: workers * SCALABILITY_TASKS_PER_WORKER,
        };
        eprintln!(
            "[scalability] cell workers={workers} sleep_ms={} tasks={} ×{} repeats (poll_ms={TUNED_POLL_MS}) …",
            spec.sleep_ms, spec.tasks, REPEATS
        );
        let median = run_cell_median_with_poll(spec, TUNED_POLL_MS).await;
        rows.push(scalability_row(spec, median));
    }
    BenchSection {
        name: "scalability · worker_n ∈ {1,2,4,8,16}, sleep=10ms".into(),
        headers: vec![
            "worker_n".into(),
            "tasks_n".into(),
            "median_wall_ms".into(),
            "tasks_per_sec".into(),
            "理论上限_tps".into(),
            "scaling_efficiency".into(),
        ],
        rows,
        notes: format!(
            "_{REPEATS}-run median · poll_ms={TUNED_POLL_MS} · 每 worker 跑 {} 条 task；\
             理论上限 = worker_n / sleep_sec；scaling_efficiency = 实测_tps / 理论_tps。\
             max_concurrency=1 隔离 per-worker 并发——只看跨 worker 的 controller 编排上限。_",
            SCALABILITY_TASKS_PER_WORKER
        ),
    }
}

fn scalability_row(spec: CellSpec, median_wall_ms: u128) -> Vec<String> {
    let wall_sec = median_wall_ms as f64 / 1000.0;
    let tps = if wall_sec > 0.0 {
        spec.tasks as f64 / wall_sec
    } else {
        f64::NAN
    };
    let theoretical_tps = spec.workers as f64 / (spec.sleep_ms as f64 / 1000.0);
    let efficiency = if theoretical_tps > 0.0 {
        tps / theoretical_tps * 100.0
    } else {
        f64::NAN
    };
    eprintln!(
        "[scalability]   workers={} wall_ms={median_wall_ms} tps={tps:.2} 理论={theoretical_tps:.2} efficiency={efficiency:.1}%",
        spec.workers
    );
    vec![
        spec.workers.to_string(),
        spec.tasks.to_string(),
        median_wall_ms.to_string(),
        format!("{tps:.2}"),
        format!("{theoretical_tps:.2}"),
        format!("{efficiency:.1}%"),
    ]
}

/// 主入口——run_baseline.rs 调本函数收集 throughput section。
pub async fn run() -> BenchSection {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(MATRIX.len() + BONUS_MATRIX.len());
    for spec in MATRIX {
        eprintln!(
            "[throughput] cell workers={} sleep_ms={} tasks={} ×{} repeats (poll_ms={TUNED_POLL_MS}) …",
            spec.workers, spec.sleep_ms, spec.tasks, REPEATS
        );
        let median = run_cell_median(*spec).await;
        rows.push(cell_row(*spec, median));
    }
    // bonus 对比：default poll_ms=50 在同 1 worker × 3 sleep 档下的损耗——
    // 让"为什么 tuned 5ms"有数字证据，不是 caveat 文字。row 的 worker_n 列加
    // (default-poll) 后缀做视觉区分。
    for spec in BONUS_MATRIX {
        eprintln!(
            "[throughput] bonus cell workers={} sleep_ms={} tasks={} ×{} repeats (poll_ms={DEFAULT_POLL_MS}) …",
            spec.workers, spec.sleep_ms, spec.tasks, REPEATS
        );
        let median = run_cell_median_with_poll(*spec, DEFAULT_POLL_MS).await;
        let mut row = cell_row(*spec, median);
        row[0] = format!("{} (poll=50)", spec.workers);
        rows.push(row);
    }
    BenchSection {
        name: "dist_throughput".into(),
        headers: vec![
            "worker_n".into(),
            "job_sleep".into(),
            "tasks_n".into(),
            "median_wall_ms".into(),
            "tasks_per_sec".into(),
            "理论上限_tps".into(),
            "fuxi_损耗".into(),
        ],
        rows,
        notes: format!(
            "_{REPEATS}-run median · max_concurrency=1（隔离 per-worker 并发）· 默认行 poll_ms={TUNED_POLL_MS} (tuned) · 末 3 行 `(poll=50)` 为 default config 对比。**实测发现**：sustained throughput 场景（queue 持续有 job → worker 不进 idle）下 poll_ms 几乎不影响损耗——10ms-sleep cell 损耗 ~20% 主要来自 axum HTTP 往返 + tokio 调度，不是 worker idle wait。tuning poll_ms 仅对 burst-then-quiet 场景有意义。_"
        ),
    }
}
