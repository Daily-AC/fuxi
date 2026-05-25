//! γ path：latency baseline bench section。
//!
//! 两条独立测量：
//!
//! - **A. e2e task dispatch latency**：1 worker (max_concurrency=1, sleep=10ms)，
//!   串行 enqueue 一条 + 等单条 done，记录 t_enqueue → t_done wall-clock。
//!   并发=1 是关键——并发会把 latency 跟 throughput 混在一起，这里只测单条
//!   全链路延迟（pull/dispatch/run/report 一周）。
//!
//! - **B. cross-node 事件流 latency**：worker 已 register，直接走 HTTP POST
//!   `/dist/event` 一条事件 → controller bus republish → home 端 subscribe
//!   收到。同进程跑 `Instant::now()` 取 publish 与 receive 时刻，差值即
//!   全链路延迟。同进程时钟一致；走 `Instant` 而不是 `Utc::now()` 是因为
//!   localhost 单条延迟 sub-ms，毫秒粒度会全砍到 0；用 micro 分辨率再以
//!   小数 ms 展示。
//!
//! 不走 NetworkBusClient：那一层带 200ms tick + batch=16 攒批，会把单条
//! latency 折叠进批次延迟。本 bench 测的是"transport+republish 自身"的
//! baseline，batcher 行为应单独 bench。
//!
//! sample 数 500——p99 落到 sample[495]，比 200 (p99=sample[198]) 单点波动
//! 小一倍。500 × 单 case 跑时间在秒级可接受。
//!
//! percentile：sort + index `samples[n*p/100]`。无插值，floor 即可，baseline
//! 数字大概准就够。

use super::{BenchSection, spawn_controller_with_workers};
use crate::dist::{DistEventReq, DistEventResp};
use futures_util::StreamExt;
use fuxi_core::{Event, EventKind, EventMeta};
use std::time::{Duration, Instant};

/// p99 收敛 + 单 case ~秒级跑时间的折中。改大让 p99 更稳，但 wall time 线性涨。
const SAMPLES: usize = 500;

/// A 路单条 task 的 sleep_ms——比真 LLM job 快 1000×，让 200 sample 不至于
/// 把 bench 拖到分钟级，同时 ≥ tokio 调度噪声地板。
const TASK_SLEEP_MS: u64 = 10;

/// A 路单条 e2e 总超时。10ms sleep + register/pull/report ~50ms upper bound，
/// 给 5s 防 axum/tokio 偶发抖动。
const TASK_DEADLINE: Duration = Duration::from_secs(5);

/// B 路单 event 全链路超时。POST + bus publish + broadcast deliver 应 sub-100ms，
/// 留 5s 防偶发卡顿。
const EVENT_DEADLINE: Duration = Duration::from_secs(5);

/// 主入口——`benches/run_baseline.rs` 调本函数收集 latency section。
pub async fn run() -> BenchSection {
    let mut rows: Vec<Vec<String>> = Vec::new();

    // ── A 路：e2e task dispatch latency ─────────────────────────────────────
    let task_samples = measure_task_dispatch_latency().await;
    rows.push(stats_row("task_dispatch", &task_samples));

    // ── B 路：事件流 latency ────────────────────────────────────────────────
    let event_samples = measure_event_flow_latency().await;
    rows.push(stats_row("event_flow", &event_samples));

    // ── thesis-v3 dump：把 raw samples 落到 CSV，给 eCDF 出图用 ────────────
    // 摘要 stats（min/p50/p99/max）丢失了分布形状；论文 fig 5-4/5-5 需要 eCDF。
    dump_samples_csv(&task_samples, &event_samples);

    let task_tail = tail_ratio(&task_samples);
    let event_tail = tail_ratio(&event_samples);
    let notes = format!(
        "_sample_n={SAMPLES}; A=串行 enqueue→done (sleep={TASK_SLEEP_MS}ms, max_concurrency=1); \
         B=直接 HTTP POST /dist/event 串行 publish→home subscribe receive。\
         long-tail (p99/p50)：task_dispatch={task_tail:.1}× / event_flow={event_tail:.1}×_"
    );

    BenchSection {
        name: "Latency · e2e task + cross-node event (γ path)".into(),
        headers: vec![
            "metric".into(),
            "sample_n".into(),
            "min_ms".into(),
            "p50_ms".into(),
            "p99_ms".into(),
            "max_ms".into(),
        ],
        rows,
        notes,
    }
}

/// A 路：起 1 worker (max_concurrency=1, sleep=10ms)，串行 enqueue 单条
/// task + 等 done，记录 t_enqueue → t_done。返回 micro 秒 sample。
async fn measure_task_dispatch_latency() -> Vec<u128> {
    let harness = spawn_controller_with_workers(1, 1, TASK_SLEEP_MS).await;

    // 先 warm 一条让 worker 完成首次 register/pull 的 cold path（约 50-100ms）。
    // bench 主 loop 起步前 worker 已稳态——前几条 sample 不会被 cold start 污染。
    let warm = harness
        .ctrl
        .enqueue(
            "bench".into(),
            "warm".into(),
            String::new(),
            None,
            vec![],
            None,
            String::new(),
            vec![],
            None,
            None,
        )
        .await;
    wait_one(&harness, &warm, TASK_DEADLINE).await;

    let mut samples = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let t_start = Instant::now();
        let job_id = harness
            .ctrl
            .enqueue(
                "bench".into(),
                format!("lat-{i}"),
                String::new(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        wait_one(&harness, &job_id, TASK_DEADLINE).await;
        samples.push(t_start.elapsed().as_micros());
    }

    harness.shutdown();
    samples
}

/// 等单条 job done。`BenchHarness::wait_until_done` 用 20ms poll，对 10ms-sleep
/// 的 task 有量化噪声；这里用 1ms 让单条 latency 测量精度提到 ±1ms。
async fn wait_one(harness: &super::BenchHarness, job_id: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if harness.ctrl.job_status(job_id).await.done {
            return;
        }
        if Instant::now() > deadline {
            panic!("latency bench: job {job_id} not done within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// B 路：起 controller + 1 worker（worker 不做活，只占用 register slot 让
/// `/dist/event` 接受其 node_id），从 home bus 端 subscribe；publish 前后
/// 各取 `Instant::now()` 算 publish→receive 全链路。返回 micro 秒 sample。
async fn measure_event_flow_latency() -> Vec<u128> {
    // sleep_ms=0 表示 worker 不做活——本路径不需要 task pull。
    let harness = spawn_controller_with_workers(1, 1, 0).await;
    // worker 已在 spawn_controller_with_workers 内完成 register；
    // 但 register 是 worker→controller HTTP，async 起飞，给一窗口让首次
    // register 落到 controller nodes 表里（否则 /dist/event 第一条 403 拒绝）。
    let node_id = "bench-node-0".to_string();
    wait_node_registered(&harness, &node_id, Duration::from_secs(2)).await;

    let bus = harness.ctrl.bus().clone();
    let mut stream = bus.subscribe();

    let http = reqwest::Client::new();
    let url = format!("{}/dist/event", harness.base_url);

    // 串行：每轮发 1 条事件并等 home 端收到对应 AgentResponded（按 text 区分），
    // 这样测的是 publish→receive **per event** 全链路；并发发会让 latency 跟
    // pipeline 行为耦合，污染单条延迟。
    let mut samples = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let marker = format!("lat-evt-{i}");
        let event = Event {
            meta: EventMeta::now(),
            kind: EventKind::AgentResponded {
                text: marker.clone(),
                artifact_ref: None,
            },
        };

        let req = DistEventReq {
            node_id: node_id.clone(),
            events: vec![event],
        };
        // t_publish 取 POST 调用前那一刻——worker 端"决定 send 一条事件"的语义起点。
        let t_publish = Instant::now();
        let resp: DistEventResp = http
            .post(&url)
            .json(&req)
            .send()
            .await
            .expect("post /dist/event")
            .json()
            .await
            .expect("decode");
        assert_eq!(resp.accepted, 1, "controller should accept registered node");

        // 等到带 marker 的 AgentResponded 抵达——其他 EventKind（dist_job_*
        // 的 Custom）跳过；marker 唯一保证我们对的是同一条。
        let deadline = Instant::now() + EVENT_DEADLINE;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            if remain.is_zero() {
                panic!("event_flow bench: marker {marker} not received within {EVENT_DEADLINE:?}");
            }
            match tokio::time::timeout(remain, stream.next()).await {
                Ok(Some(Ok(ev))) => match ev.kind {
                    EventKind::AgentResponded { ref text, .. } if text == &marker => {
                        samples.push(t_publish.elapsed().as_micros());
                        break;
                    }
                    _ => continue,
                },
                Ok(Some(Err(_))) | Ok(None) => continue,
                Err(_) => {
                    panic!("event_flow bench: timeout waiting for marker {marker}");
                }
            }
        }
    }

    harness.shutdown();
    samples
}

/// register 是 worker→controller 异步 HTTP，给本节点至 controller `nodes`
/// 表落库一窗口；探测 `node_info(node_id)` 是否非 None。
async fn wait_node_registered(harness: &super::BenchHarness, node_id: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if harness.ctrl.node_info(node_id).await.is_some() {
            return;
        }
        if Instant::now() > deadline {
            panic!("bench-node {node_id} not registered within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// thesis-v3 raw samples dump——CSV 给 matplotlib eCDF 用。
/// 失败只 log warn 不 panic：bench 报告才是主交付，dump 只是副产物。
fn dump_samples_csv(task_us: &[u128], event_us: &[u128]) {
    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_dir = crate_root.join("../../deliverables/thesis-v3/benchmarks");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("[latency] mkdir thesis-v3/benchmarks 失败：{e}（跳过 CSV dump）");
        return;
    }
    let path = out_dir.join("latency-samples.csv");
    let mut csv = String::from("metric,sample_us\n");
    for s in task_us {
        csv.push_str(&format!("task_dispatch,{}\n", s));
    }
    for s in event_us {
        csv.push_str(&format!("event_flow,{}\n", s));
    }
    if let Err(e) = std::fs::write(&path, csv) {
        eprintln!("[latency] dump CSV 失败：{e}");
    } else {
        eprintln!("[latency] raw samples dumped → {}", path.display());
    }
}

/// 单 sample 集合 → 报告 row：metric / n / min / p50 / p99 / max。
/// sample 单位 micro 秒；输出列保 `_ms`，值用 2 位小数 ms 渲染——避免
/// localhost sub-ms latency 被毫秒整数 floor 砍 0。
fn stats_row(metric: &str, samples_us: &[u128]) -> Vec<String> {
    let mut sorted = samples_us.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let min = *sorted.first().expect("non-empty");
    let max = *sorted.last().expect("non-empty");
    let p50 = sorted[n * 50 / 100];
    let p99 = sorted[(n * 99 / 100).min(n - 1)];
    vec![
        metric.to_string(),
        n.to_string(),
        fmt_us_as_ms(min),
        fmt_us_as_ms(p50),
        fmt_us_as_ms(p99),
        fmt_us_as_ms(max),
    ]
}

fn fmt_us_as_ms(us: u128) -> String {
    format!("{:.2}", us as f64 / 1000.0)
}

/// p99/p50 长尾比——note 里用来一眼看分布是否畸重。p50=0 时返 0（避免除零）。
fn tail_ratio(samples_us: &[u128]) -> f64 {
    let mut s = samples_us.to_vec();
    s.sort_unstable();
    let n = s.len();
    let p50 = s[n * 50 / 100] as f64;
    let p99 = s[(n * 99 / 100).min(n - 1)] as f64;
    if p50 <= 0.0 { 0.0 } else { p99 / p50 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// percentile 算法白盒：n=100 升序 sample（值=0..100 micros），p50/p99
    /// 落正确 index。fmt 把 us 转 ms 保 2 位小数。
    #[test]
    fn stats_row_indexes_p50_p99_correctly() {
        let samples: Vec<u128> = (0..100).collect();
        let row = stats_row("test", &samples);
        // [metric, n, min, p50, p99, max]
        assert_eq!(row[0], "test");
        assert_eq!(row[1], "100");
        assert_eq!(row[2], "0.00", "min = 0us = 0.00ms");
        assert_eq!(row[3], "0.05", "p50 = 50us = 0.05ms");
        assert_eq!(row[4], "0.10", "p99 = 99us = 0.10ms"); // 99/1000=0.099 → "0.10"
        assert_eq!(row[5], "0.10", "max = 99us → 0.10ms");
    }

    /// n=200 时 p99 应落 sample[198]，符合 task spec 注释。
    #[test]
    fn stats_row_p99_at_index_198_for_n_200() {
        // sample[i] = i * 1000 us（即 i ms），sort 后 sample[198] = 198_000 us = 198ms
        let mut samples: Vec<u128> = (0..200u128).map(|i| i * 1000).collect();
        samples.reverse(); // 让排序成必要
        let row = stats_row("x", &samples);
        assert_eq!(row[4], "198.00", "n=200 时 p99 = sample[198] = 198ms");
    }

    /// p50=0 时 tail_ratio 返 0 不 panic。
    #[test]
    fn tail_ratio_handles_zero_p50() {
        let samples: Vec<u128> = vec![0; 100];
        assert_eq!(tail_ratio(&samples), 0.0);
    }

    /// 长尾比例：p99 远大于 p50 时返合理倍数。
    #[test]
    fn tail_ratio_computes_long_tail() {
        let mut samples: Vec<u128> = vec![10; 99];
        samples.push(1000); // 单个长尾，p99 应抓到 1000
        let r = tail_ratio(&samples);
        // n=100, p99 落 sample[99]=1000, p50=sample[50]=10 → 100×
        assert!((r - 100.0).abs() < 0.01, "tail ratio ≈ 100×, got {r}");
    }
}
