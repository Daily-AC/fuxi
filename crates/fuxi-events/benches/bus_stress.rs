//! TB.4 事件总线纯压测——隔离通信层性能，对应论文 §5.6 + 公式 (3-1)。
//!
//! 设计：
//! - 单 publisher 直调 `EventBus::publish`
//! - N 个 subscriber 各自 `subscribe()` 接收
//! - 维度 1：N ∈ {1, 4, 16, 64} subscriber
//! - 维度 2：发送速率 1k/10k/100k events/s
//! - 维度 3：payload size 小（256B）vs 大（4 KiB）
//!
//! 跑法：`cargo bench -p fuxi-events --bench bus_stress`
//!
//! 输出 markdown 写入 `deliverables/thesis-v2/benchmarks/bus-stress/results.md`。
//!
//! ## 实现取舍
//!
//! - **不走 SQLite 持久化**：本 bench 关心 broadcast fan-out + subscriber
//!   消费链路自身性能；持久化 writer 是另一根管子（mpsc → SQLite WAL）。
//!   `with_memory_store()` 提供 `:memory:` SQLite，writer 走最快路径——
//!   不会成为 bench 瓶颈，但也不污染 broadcast 数字。
//! - **drops 计数 = 发送数 - 接收数**：`EventBus::subscribe()` 返回的
//!   `EventStream` 内部已把 `RecvError::Lagged` filter 掉（warn-then-skip），
//!   消费端拿不到 Lagged 计数。两边对账更直观。
//! - **延迟测量走 payload 内嵌时间戳**：side-channel `Mutex<Vec<Instant>>`
//!   在 100k/s × 64 sub 并发读下锁竞争太重；`Custom { payload: { t_send_ns } }`
//!   自包含，sub 端 `Instant::now() - t_send` 直出延迟，无锁。
//! - **payload size 控制**：在 payload JSON 里塞 `String::from('x'.repeat(N))`。
//!   small=256B 一字段；large=4 KiB（论文里对应 AssistantText 单 chunk
//!   大致量级）。
//! - **publish 控速**：100k/s 时 `tokio::time::interval(10us)` macOS 上不准；
//!   改 busy-loop spin 比对 `Instant::now()` ≥ 下一发送点。微秒级精度够。

use futures_util::StreamExt;
use fuxi_core::{Event, EventKind, EventMeta};
use fuxi_events::EventBus;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// 单 cell 跑 5 秒的稳态时长——足够 100k/s 收集 500k 个样本，p99 收敛；
/// 但又不会让 bench wall clock 拖到分钟级（4×3×2 = 24 cell）。
const RUN_SECS: u64 = 5;

/// 收尾等所有 subscriber drain 的最长时长——防止 publisher 发完最后一条
/// 但 broadcast pipe 还在传时被截。100k/s × 5 sec = 500k 条 worst case
/// drain 应在 1 秒内（broadcast 走 in-process，receiver 后台消费）。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// 发送方加大 broadcast buffer——默认 1024 在 100k/s × 64 sub 下任何
/// 微小消费抖动就触发 Lagged 海啸，drop 数把 publish_tps 信号埋掉。
/// 32k slots 给慢 sub 留 ~320ms 的容忍窗口，让 drops 反映"真持续不上"
/// 而不是首秒过载。
const BROADCAST_BUFFER: usize = 32_768;

#[derive(Clone, Copy)]
struct CellSpec {
    n_sub: usize,
    rate: u64,
    payload_label: &'static str,
    payload_size: usize,
}

struct CellResult {
    publish_total: u64,
    publish_tps: f64,
    recv_p50_us: f64,
    recv_p99_us: f64,
    drops: u64,
}

fn cell_matrix() -> Vec<CellSpec> {
    let n_subs = [1usize, 4, 16, 64];
    let rates = [1_000u64, 10_000, 100_000];
    let payloads: [(&str, usize); 2] = [("small", 256), ("large", 4096)];
    let mut out = Vec::with_capacity(n_subs.len() * rates.len() * payloads.len());
    for &n_sub in &n_subs {
        for &rate in &rates {
            for &(label, size) in &payloads {
                out.push(CellSpec {
                    n_sub,
                    rate,
                    payload_label: label,
                    payload_size: size,
                });
            }
        }
    }
    out
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    eprintln!("[bus_stress] 开跑 4×3×2 = 24 cell，每 cell {RUN_SECS}s 稳态 …");

    let mut markdown = String::new();
    markdown.push_str("# 事件总线纯压测（TB.4）\n\n");
    markdown.push_str(&format!(
        "_Generated at: {}_\n\n",
        chrono::Utc::now().to_rfc3339()
    ));
    markdown.push_str(&format!(
        "维度：N subscriber × 发送速率 × payload size。每 cell 持续 {RUN_SECS}s 稳态。\
         broadcast buffer = {BROADCAST_BUFFER} slots（默认 1024 在 100k/s 下首秒就溢出）。\n\n"
    ));
    markdown.push_str("| subscribers | rate (ev/s) | payload | publish_tps | recv_p50_us | recv_p99_us | drops |\n");
    markdown.push_str("|---|---|---|---|---|---|---|\n");

    for spec in cell_matrix() {
        eprintln!(
            "[bus_stress] cell n_sub={} rate={} payload={}({} B) …",
            spec.n_sub, spec.rate, spec.payload_label, spec.payload_size
        );
        let r = run_cell(spec).await;
        eprintln!(
            "[bus_stress]   publish_total={} publish_tps={:.1} p50={:.1}us p99={:.1}us drops={}",
            r.publish_total, r.publish_tps, r.recv_p50_us, r.recv_p99_us, r.drops
        );
        markdown.push_str(&format!(
            "| {} | {} | {} | {:.1} | {:.1} | {:.1} | {} |\n",
            spec.n_sub,
            spec.rate,
            spec.payload_label,
            r.publish_tps,
            r.recv_p50_us,
            r.recv_p99_us,
            r.drops
        ));
    }

    markdown.push_str("\n## 观察\n\n");
    markdown.push_str(
        "见 `deliverables/thesis-v2/benchmarks/v2-2026-05-07.md` §4 汇总分析。\
         核心数字：低速率（1k/10k）任何 N 下 drop=0、p99 维持微秒级；\
         100k/s + 64 sub 是极端组合，broadcast buffer 才出现持续不上 → drops。\
         payload 大小对 p50 影响有限（broadcast 是 Arc clone，不复制 byte），\
         主要影响 p99 长尾（receiver 端 deserialize 大 JSON 的额外抖动）。\n",
    );

    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = crate_root.join("../../deliverables/thesis-v2/benchmarks/bus-stress/results.md");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir bus-stress");
    }
    std::fs::write(&path, markdown).expect("写 bus_stress 报告失败");
    eprintln!("[bus_stress] 报告写入 {}", path.display());
}

async fn run_cell(spec: CellSpec) -> CellResult {
    // 1) bus 单独构造——每 cell 一个新 bus，避免上一 cell 的订阅残留 / 缓冲污染。
    let bus = EventBus::new(
        fuxi_events::EventStore::connect_memory()
            .await
            .expect("memory store"),
        fuxi_events::EventBusConfig {
            buffer: BROADCAST_BUFFER,
            ..Default::default()
        },
    );

    // 2) 起 N 个 subscriber——每个独立 task 持续 recv，记录 (recv_us - send_us)。
    //    每 sub 的 payload received 计数也回报，用于跟 publish_total 对账算 drops。
    let stop_flag = Arc::new(AtomicU64::new(0));
    let payload_pad = std::sync::Arc::new(make_payload(spec.payload_size));

    let mut sub_handles = Vec::with_capacity(spec.n_sub);
    for _ in 0..spec.n_sub {
        let mut stream = bus.subscribe();
        let stop = stop_flag.clone();
        let h = tokio::spawn(async move {
            let mut latencies_us: Vec<u64> = Vec::with_capacity(2_000_000);
            let mut received: u64 = 0;
            // recv loop——直到 publisher 发出停止信号且管道 drain 完。
            //
            // EventStream 已 filter Lagged → drops 在 publisher 端
            // (published - received) 体现，sub 端不需要识别 lag 错误。
            loop {
                if stop.load(Ordering::Relaxed) == 1 {
                    // 进入 drain 阶段——再尝试 timeout 拉一段，把 broadcast 在
                    // publisher 停发后还在 in-flight 的事件吃完，否则 drop 数虚高。
                    let drain_deadline = Instant::now() + DRAIN_TIMEOUT;
                    while Instant::now() < drain_deadline {
                        match tokio::time::timeout(Duration::from_millis(50), stream.next()).await {
                            Ok(Some(Ok(ev))) => {
                                if let Some(lat) = decode_latency_us(&ev) {
                                    latencies_us.push(lat);
                                    received += 1;
                                }
                            }
                            Ok(Some(Err(_))) => continue,
                            Ok(None) | Err(_) => break,
                        }
                    }
                    break;
                }
                match tokio::time::timeout(Duration::from_millis(20), stream.next()).await {
                    Ok(Some(Ok(ev))) => {
                        if let Some(lat) = decode_latency_us(&ev) {
                            latencies_us.push(lat);
                            received += 1;
                        }
                    }
                    Ok(Some(Err(_))) => continue,
                    Ok(None) => break,
                    Err(_) => continue, // 心跳超时，回循环顶看 stop。
                }
            }
            (latencies_us, received)
        });
        sub_handles.push(h);
    }

    // 给 subscriber 启动一窗口——broadcast::subscribe() 是同步注册，但
    // tokio::spawn 后还没轮到这些 task 跑。100us 让它们都进 recv loop。
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 3) publisher 控速发 RUN_SECS × rate 条事件——busy-loop 比 interval 准。
    let total_target: u64 = spec.rate * RUN_SECS;
    let interval_ns: u64 = 1_000_000_000 / spec.rate;
    let bus_pub = bus.clone();
    let pad = payload_pad.clone();
    let publish_handle = tokio::task::spawn_blocking(move || -> u64 {
        // Instant 控速（单调时钟、不会被 NTP 跳）；t_send_ns 用 SystemTime
        // 从 UNIX epoch 起的纳秒——subscriber 端同样读 SystemTime 才能跨
        // tokio task 算延迟（Instant 跨线程虽然底层一致，但语义不保证）。
        let start = Instant::now();
        let mut next = start;
        let mut sent: u64 = 0;
        for _ in 0..total_target {
            // busy-spin 到下一发送时刻。100k/s = 10us 间隔，sleep 系统调用噪声
            // 在 macOS 是 50-500us 量级会把发送速率打到 ~2k/s 上限——只能 spin。
            loop {
                let now = Instant::now();
                if now >= next {
                    break;
                }
                // 间隔 ≥ 50us 时让出调度避免饿死其它 task；<50us 全 spin。
                if next.saturating_duration_since(now) > Duration::from_micros(50) {
                    std::thread::yield_now();
                } else {
                    std::hint::spin_loop();
                }
            }
            let t_send_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let ev = make_event(t_send_ns, &pad);
            // bus.publish 失败语义只在 writer 死时——bench 路径不应触发。
            // Err 时停发让上层 publish_total 反映真发送数。
            if bus_pub.publish(ev).is_err() {
                break;
            }
            sent += 1;
            next += Duration::from_nanos(interval_ns);
        }
        sent
    });

    let publish_total = publish_handle.await.expect("publish join");
    // 4) 通知 subscriber 进入 drain 阶段。
    stop_flag.store(1, Ordering::Relaxed);

    // 5) 收齐所有 sub 数据。
    let mut all_lat: Vec<u64> = Vec::new();
    let mut total_received: u64 = 0;
    for h in sub_handles {
        let (lats, n) = h.await.expect("sub join");
        total_received += n;
        all_lat.extend(lats);
    }

    let publish_tps = publish_total as f64 / RUN_SECS as f64;
    // drops = N_sub × published - received_total
    // 每个 sub 期望接收 publish_total 条；总 drops = sub × pub - sum(received)
    let expected_total: u64 = publish_total * spec.n_sub as u64;
    let drops = expected_total.saturating_sub(total_received);
    let (p50, p99) = if all_lat.is_empty() {
        (f64::NAN, f64::NAN)
    } else {
        all_lat.sort_unstable();
        let n = all_lat.len();
        let p50 = all_lat[n * 50 / 100] as f64;
        let p99 = all_lat[(n * 99 / 100).min(n - 1)] as f64;
        (p50 / 1000.0, p99 / 1000.0)
    };

    CellResult {
        publish_total,
        publish_tps,
        recv_p50_us: p50,
        recv_p99_us: p99,
        drops,
    }
}

/// 构造一条 Custom 事件：payload 内嵌发送时刻（纳秒，`t_send_ns`）+
/// 一个 padding 字符串撑大 size。subscribe 端走 `decode_latency_us` 还原。
fn make_event(t_send_ns: u64, padding: &str) -> Event {
    Event {
        meta: EventMeta::now(),
        kind: EventKind::Custom {
            label: "bus_stress".into(),
            payload: serde_json::json!({
                "t_send_ns": t_send_ns,
                "pad": padding,
            }),
        },
    }
}

/// 从 Custom payload 恢复发送 → 接收延迟（微秒）。
/// 解析失败（不是 bench 自己的事件）返回 None 让 sub 端跳过——其它系统事件
/// （`event_store_lagged` 哨兵等）不应进 latency 直方图。
fn decode_latency_us(ev: &Event) -> Option<u64> {
    let EventKind::Custom { label, payload } = &ev.kind else {
        return None;
    };
    if label != "bus_stress" {
        return None;
    }
    let t_send_ns = payload.get("t_send_ns")?.as_u64()?;
    // 双端都用 SystemTime（unix epoch 锚点）算延迟——Instant 跨 task 语义
    // 不保证一致，SystemTime 是机器全局时钟无歧义。NTP 跳变在 5s bench 窗口
    // 内可忽略。
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos() as u64;
    Some((now_ns.saturating_sub(t_send_ns)) / 1000)
}

/// 构造 size 字节的 padding 字符串——填 ASCII 'x'，1 byte/char。
fn make_payload(size: usize) -> String {
    "x".repeat(size)
}
