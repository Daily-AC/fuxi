# Baseline Bench Report

_Generated at: 2026-05-07T08:31:54.053835+00:00_

## poll_ms 扫描 · worker_n=4, tasks_n=400, sleep=10ms

| poll_ms | median_wall_ms | tasks_per_sec | 理论上限_tps | fuxi_损耗 |
| --- | --- | --- | --- | --- |
| 5 | 1285 | 311.28 | 400.00 | 22.2% |
| 10 | 1299 | 307.93 | 400.00 | 23.0% |
| 25 | 1242 | 322.06 | 400.00 | 19.5% |
| 50 | 1225 | 326.53 | 400.00 | 18.4% |
| 100 | 1235 | 323.89 | 400.00 | 19.0% |

_5-run median · 固定 cell `4w × 10ms × 400 tasks`，仅扫 poll_ms。理论 tps = worker_n / sleep_sec = 400._

