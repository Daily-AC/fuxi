# Baseline Bench Report

_Generated at: 2026-05-07T08:33:38.334045+00:00_

## scalability · worker_n ∈ {1,2,4,8,16}, sleep=10ms

| worker_n | tasks_n | median_wall_ms | tasks_per_sec | 理论上限_tps | scaling_efficiency |
| --- | --- | --- | --- | --- | --- |
| 1 | 100 | 1226 | 81.57 | 100.00 | 81.6% |
| 2 | 200 | 1208 | 165.56 | 200.00 | 82.8% |
| 4 | 400 | 1254 | 318.98 | 400.00 | 79.7% |
| 8 | 800 | 1197 | 668.34 | 800.00 | 83.5% |
| 16 | 1600 | 1197 | 1336.68 | 1600.00 | 83.5% |

_5-run median · poll_ms=5 · 每 worker 跑 100 条 task；理论上限 = worker_n / sleep_sec；scaling_efficiency = 实测_tps / 理论_tps。max_concurrency=1 隔离 per-worker 并发——只看跨 worker 的 controller 编排上限。_

