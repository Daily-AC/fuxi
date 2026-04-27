//! 系统级 bench 公共脚手架——给 `crates/fuxi-cli/benches/*` 用，
//! 同时导出 stub adapter / harness / 报告 helper。
//!
//! 为什么放在 lib 里：benches 是独立 crate，只能见 `pub` 表面；CliAdapter /
//! AdapterFactory / run_worker_with 是 `pub(crate)`，benches 拿不到。本模块
//! 在 lib 内消化掉这些 internals，对外只暴露"起一坨 worker + dispatch job +
//! 写 markdown 报告"的高层 API。
//!
//! 不复用 `dist::tests::StubAdapter`：那一份在 `#[cfg(test)]` 内只对单测可
//! 见，且耦合更复杂的 cancel 行为；bench 只要"sleep N ms 后返 ok"。两份独
//! 立维护，避免单测改动牵连 bench 数据。
//!
//! ## 子模块契约（β/γ 据此各自填）
//!
//! - `throughput::run() -> BenchSection` —— β path，tasks/sec 矩阵
//! - `latency::run() -> BenchSection` —— γ path，e2e dispatch + event p50/p99
//!
//! `run_baseline.rs` 主入口顺序调它们，append 到 markdown 报告。

pub mod latency;
pub mod throughput;

use crate::dist::{
    AdapterFactory, CliAdapter, DistController, DistJob, DistWorkerArgs, WorkerCtx, router,
    run_worker_with,
};
use anyhow::Result;
use fuxi_events::EventBus;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;

/// bench 用 sleep-then-ok 的 fake adapter——run() 睡 `sleep_ms` 毫秒后返 `(true, "ok")`，
/// 模拟一个 LLM job 的完成。`active` 计数共享给所有 clone，便于 bench 侧
/// "等 N 个 job 同时在跑"再开始计时（剔除 register/pull 噪音）。
pub struct StubAdapter {
    sleep_ms: u64,
    active: Arc<AtomicUsize>,
}

impl StubAdapter {
    /// 当前活跃 in-flight job 数（所有共享同一个 active 计数器的 stub 累加）。
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CliAdapter for StubAdapter {
    fn name(&self) -> &'static str {
        "stub-bench"
    }
    async fn run(&self, _ctx: &WorkerCtx<'_>, _job: &DistJob) -> Result<(bool, String)> {
        self.active.fetch_add(1, Ordering::SeqCst);
        struct Guard<'a>(&'a AtomicUsize);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _g = Guard(&self.active);
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        Ok((true, "ok".into()))
    }
}

/// 构造一个共享 active 计数的 stub adapter。
pub fn stub_adapter_with_sleep(sleep_ms: u64) -> Arc<StubAdapter> {
    Arc::new(StubAdapter {
        sleep_ms,
        active: Arc::new(AtomicUsize::new(0)),
    })
}

/// 构造一个**共享同一 active 计数**的 stub adapter——传入既有 active，让
/// bench 多 worker 共用同一个计数器观察"全局并发"。pub(crate) 内部 helper。
pub(crate) fn stub_adapter_with_shared_active(
    sleep_ms: u64,
    active: Arc<AtomicUsize>,
) -> Arc<StubAdapter> {
    Arc::new(StubAdapter { sleep_ms, active })
}

/// 把 stub adapter 包成 `AdapterFactory`（worker 主循环按 cli 名取 adapter
/// 的接口）。每次 factory 调用 clone 出新的 trait object 但共享同一份
/// `active` 计数，便于跨 worker 观察并发度。
///
/// `pub(crate)` 不外露——benches 只走 `spawn_controller_with_workers` 这个
/// 高层 API；factory + AdapterFactory 都是 worker 主循环的内部接口。
pub(crate) fn make_factory(adapter: Arc<StubAdapter>) -> AdapterFactory {
    Arc::new(move |_cli, _args| {
        let cloned = StubAdapter {
            sleep_ms: adapter.sleep_ms,
            active: adapter.active.clone(),
        };
        Ok(Box::new(cloned) as Box<dyn CliAdapter>)
    })
}

/// 起一个 axum dist controller 在随机端口；返回 (controller, base_url, server_handle)。
pub async fn spawn_controller() -> (Arc<DistController>, String, JoinHandle<()>) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let ctrl = Arc::new(DistController::new("bench-tok".into(), bus));
    let app = router(ctrl.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // 给 axum 一窗口完成 accept-loop ready，避免首次 POST race。
    tokio::time::sleep(Duration::from_millis(20)).await;
    (ctrl, format!("http://{addr}"), handle)
}

/// bench HMAC secret——controller 的 `router()`（无 hmac_layer）不验签，仅
/// 满足 `run_worker_with` 入参约束。
fn bench_secret() -> Arc<crate::dist_auth::HmacSecret> {
    Arc::new(crate::dist_auth::HmacSecret::new("bench".into()))
}

/// 一个 bench harness：controller + N 个 worker 已起好、共享同一 active
/// 计数。bench 侧调 `enqueue_n` 派 job，等 `wait_until_done`，然后 `shutdown`
/// 释放 server / worker handles。
pub struct BenchHarness {
    pub ctrl: Arc<DistController>,
    pub base_url: String,
    pub active: Arc<AtomicUsize>,
    server: JoinHandle<()>,
    workers: Vec<JoinHandle<()>>,
}

impl BenchHarness {
    /// 派 N 个无 tag、无 system_prompt 的 job 到 controller，返回 job_ids。
    pub async fn enqueue_n(&self, n: usize, title_prefix: &str) -> Vec<String> {
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let id = self
                .ctrl
                .enqueue(
                    "bench".into(),
                    format!("{title_prefix}-{i}"),
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
            ids.push(id);
        }
        ids
    }

    /// 阻塞等到所有 job_ids 都 done；超时 panic。
    pub async fn wait_until_done(&self, job_ids: &[String], timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut all_done = true;
            for id in job_ids {
                if !self.ctrl.job_status(id).await.done {
                    all_done = false;
                    break;
                }
            }
            if all_done {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "bench harness: {} jobs not done within {timeout:?}",
                    job_ids.len()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// abort server + worker tasks——bench 单 case 收尾用。
    pub fn shutdown(self) {
        for w in self.workers {
            w.abort();
        }
        self.server.abort();
    }
}

/// 起一个 controller + n_workers 个 worker，每个 worker `max_concurrency=worker_concurrency`，
/// 全用 sleep_ms 的 stub adapter（共享 active 计数）。worker 拉 job 的 poll_ms 默认 20。
pub async fn spawn_controller_with_workers(
    n_workers: usize,
    worker_concurrency: u32,
    sleep_ms: u64,
) -> BenchHarness {
    spawn_controller_with_workers_tuned(n_workers, worker_concurrency, sleep_ms, 20).await
}

/// 同 `spawn_controller_with_workers` 但显式选 `poll_ms`——throughput bench
/// 用 5ms 把 worker idle wait 摊薄到不污染 fuxi infrastructure overhead 数字
/// （默认 50ms 在 10ms sleep job 场景会让 idle wait 主导 wall_time）。
pub async fn spawn_controller_with_workers_tuned(
    n_workers: usize,
    worker_concurrency: u32,
    sleep_ms: u64,
    poll_ms: u64,
) -> BenchHarness {
    let (ctrl, base, server) = spawn_controller().await;
    let active = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::with_capacity(n_workers);
    for i in 0..n_workers {
        let stub = stub_adapter_with_shared_active(sleep_ms, active.clone());
        let factory = make_factory(stub);
        let args = DistWorkerArgs {
            controller: base.clone(),
            node: format!("bench-node-{i}"),
            token: Some("bench-tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms,
            tags: vec![],
            max_concurrency: worker_concurrency,
        };
        let secret = bench_secret();
        let handle = tokio::spawn(async move {
            let _ = run_worker_with(
                args,
                "bench-tok".into(),
                secret,
                factory,
                Duration::from_millis(200),
            )
            .await;
        });
        workers.push(handle);
    }
    BenchHarness {
        ctrl,
        base_url: base,
        active,
        server,
        workers,
    }
}

// ── chaos test helpers (path 4 β) ─────────────────────────────────────────
//
// 这些 helper 给 `crates/fuxi-cli/tests/chaos_resilience.rs` 用。原本是
// `dist::tests` 内部辅具，但 chaos 测试要在 lib 外做"起单 worker → abort →
// 验另一 worker 接管"，必须从 integration test 拿到独立 JoinHandle。把生产
// `run_worker_with` 的 wrap 集中放 bench_support，避免每个 integration test
// 重新拼一遍 args + factory + secret。

/// 起单个 worker（sleep_ms stub adapter）连到指定 controller，返回**独立**
/// JoinHandle 给测试 abort 用——不像 `BenchHarness::shutdown` 一锅端，chaos
/// 测试常需要"abort worker_a 但 worker_b 继续跑"。
///
/// `tags` 给 worker 声明；`max_concurrency` 给 register。`heartbeat_ms` 控制
/// 心跳间隔——chaos 测试要能在合理时间窗内观察到 last_seen 老化，把它降到
/// 100ms 让 sweep 走得快。
pub fn spawn_one_worker(
    base_url: String,
    node_id: String,
    sleep_ms: u64,
    tags: Vec<String>,
    max_concurrency: u32,
    heartbeat_ms: u64,
) -> JoinHandle<()> {
    let stub = stub_adapter_with_sleep(sleep_ms);
    let factory = make_factory(stub);
    let args = DistWorkerArgs {
        controller: base_url,
        node: node_id,
        token: Some("bench-tok".into()),
        codex_bin: "codex".into(),
        cc_bin: "claude".into(),
        poll_ms: 30,
        tags,
        max_concurrency,
    };
    let secret = bench_secret();
    tokio::spawn(async move {
        let _ = run_worker_with(
            args,
            "bench-tok".into(),
            secret,
            factory,
            Duration::from_millis(heartbeat_ms),
        )
        .await;
    })
}

/// 同 `spawn_one_worker`，但所有 worker 共享外部传入的 active 计数器——
/// chaos #4（多 worker × 多 job）要观测"全局 N 个 worker 同时在跑"做 kill 时机。
pub fn spawn_one_worker_shared_active(
    base_url: String,
    node_id: String,
    sleep_ms: u64,
    active: Arc<AtomicUsize>,
    heartbeat_ms: u64,
) -> JoinHandle<()> {
    let stub = stub_adapter_with_shared_active(sleep_ms, active);
    let factory = make_factory(stub);
    let args = DistWorkerArgs {
        controller: base_url,
        node: node_id,
        token: Some("bench-tok".into()),
        codex_bin: "codex".into(),
        cc_bin: "claude".into(),
        poll_ms: 30,
        tags: vec![],
        max_concurrency: 1,
    };
    let secret = bench_secret();
    tokio::spawn(async move {
        let _ = run_worker_with(
            args,
            "bench-tok".into(),
            secret,
            factory,
            Duration::from_millis(heartbeat_ms),
        )
        .await;
    })
}

/// 在 worker 与真 controller 之间插入"全路由阻断"代理。
/// `partition` 切到 true 时**所有** `/dist/*` 都返 503，模拟链路彻底中断；
/// 否则透传给上游。返回 `(proxy_url, partition_flag, axum_serve_handle)`。
///
/// chaos 测试用此 fixture 验"网络抖动 < sweep timeout 不丢 job"
/// + "partition > sweep timeout 后 worker 能否复活"两条路径。
///
/// 实现复用 reqwest::Client 转发——避免重写 axum body 解析。`Host` 头剔掉，
/// 让上游 client 重设；其它 header（HMAC sig / nonce 等）原样转。
pub async fn spawn_partition_proxy(
    upstream: String,
) -> (String, Arc<std::sync::atomic::AtomicBool>, JoinHandle<()>) {
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::IntoResponse;
    use axum::{Router, routing::any};
    use reqwest::Client;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone)]
    struct ProxyState {
        upstream: String,
        client: Client,
        partition: Arc<AtomicBool>,
    }

    let partition = Arc::new(AtomicBool::new(false));
    let state = ProxyState {
        upstream,
        client: Client::new(),
        partition: partition.clone(),
    };

    async fn forward(
        State(st): State<ProxyState>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> axum::response::Response {
        if st.partition.load(Ordering::SeqCst) {
            return (StatusCode::SERVICE_UNAVAILABLE, "partition").into_response();
        }
        let path_q = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        let url = format!("{}{}", st.upstream, path_q);
        let mut rb = st.client.request(method, &url);
        for (k, v) in headers.iter() {
            if k.as_str().eq_ignore_ascii_case("host") {
                continue;
            }
            rb = rb.header(k, v);
        }
        let resp = match rb.body(body).send().await {
            Ok(r) => r,
            Err(e) => {
                return (StatusCode::BAD_GATEWAY, format!("proxy upstream err: {e}"))
                    .into_response();
            }
        };
        let status = resp.status();
        let bytes = resp.bytes().await.unwrap_or_default();
        (status, bytes).into_response()
    }

    let app = Router::new().fallback(any(forward)).with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("partition proxy bind");
    let addr = listener.local_addr().expect("partition proxy addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    (format!("http://{addr}"), partition, handle)
}

// ── markdown 报告 ──────────────────────────────────────────────────────────

/// 一段 bench 报告：标题 + 表头 + 数据行 + 备注。`rows[i].len()` 应等于
/// `headers.len()`——`write_baseline_report` 不强检，调用方自检。
pub struct BenchSection {
    pub name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub notes: String,
}

/// 把多个 BenchSection 拼成 markdown 写入 `path`。
///
/// 输出格式（β/γ 据此填）：
///
/// ```markdown
/// # Baseline Bench Report
///
/// _Generated at: <timestamp>_
///
/// ## <section name>
///
/// | h1 | h2 | ... |
/// | --- | --- | --- |
/// | r1c1 | r1c2 | ... |
///
/// <notes>
/// ```
pub fn write_baseline_report(path: &Path, sections: &[BenchSection]) -> std::io::Result<()> {
    let mut out = String::new();
    out.push_str("# Baseline Bench Report\n\n");
    out.push_str(&format!(
        "_Generated at: {}_\n\n",
        chrono::Utc::now().to_rfc3339()
    ));
    for sec in sections {
        out.push_str(&format!("## {}\n\n", sec.name));
        if !sec.headers.is_empty() {
            out.push_str("| ");
            out.push_str(&sec.headers.join(" | "));
            out.push_str(" |\n| ");
            out.push_str(
                &sec.headers
                    .iter()
                    .map(|_| "---")
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
            out.push_str(" |\n");
            for row in &sec.rows {
                out.push_str("| ");
                out.push_str(&row.join(" | "));
                out.push_str(" |\n");
            }
            out.push('\n');
        }
        if !sec.notes.is_empty() {
            out.push_str(&sec.notes);
            out.push_str("\n\n");
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// stub adapter sleep 指定时长后返 (true, "ok")——bench 测时基线。
    ///
    /// **不端到端起 axum + worker**：那条路径已在 `dist::tests::worker_runs_two_jobs_concurrently`
    /// 通过 dist::tests 内部的 sleep stub 充分覆盖；本测试只验 bench_support
    /// 这层 helper 自身合约（构造 / active 共享 / 计数对账），避免
    /// timing-sensitive 的 dist 测试在并行 cargo test 下 flaky。
    /// 真正的 e2e 走 `cargo bench --bench run_baseline`。
    #[tokio::test]
    async fn stub_adapter_sleeps_then_returns_ok() {
        // 1) 构造态：active=0，name="stub-bench"
        let stub = stub_adapter_with_sleep(200);
        assert_eq!(stub.active_count(), 0, "构造后未跑：active=0");
        assert_eq!(CliAdapter::name(&*stub), "stub-bench");

        // 2) 共享 active：同一 Arc<AtomicUsize> 应被两份 stub clone 看到。
        // run() 需要 WorkerCtx 内部字段（pub(crate)），单测构造不便；
        // 我们直接复刻 run() 内核（+1 / sleep / -1）验 RAII Drop 对账。
        let active = Arc::new(AtomicUsize::new(0));
        let _stub_a = stub_adapter_with_shared_active(150, active.clone());
        let _stub_b = stub_adapter_with_shared_active(150, active.clone());
        assert_eq!(active.load(Ordering::SeqCst), 0, "构造不动 active");

        let h = {
            let a = active.clone();
            tokio::spawn(async move {
                a.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(120)).await;
                a.fetch_sub(1, Ordering::SeqCst);
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(active.load(Ordering::SeqCst) >= 1, "in-flight 时 active≥1");
        h.await.expect("join");
        assert_eq!(active.load(Ordering::SeqCst), 0, "完成后 active=0");
    }

    /// markdown 报告写到 tempdir 后读回比对：含标题、headers、rows、notes、
    /// 自动建父目录。β/γ 据此格式填 section。
    #[test]
    fn write_baseline_report_writes_well_formed_markdown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("subdir/baseline-test.md");
        let sections = vec![
            BenchSection {
                name: "Throughput".into(),
                headers: vec!["workers".into(), "tasks/sec".into()],
                rows: vec![
                    vec!["1".into(), "9.8".into()],
                    vec!["4".into(), "39.2".into()],
                ],
                notes: "_sleep=100ms × 50 jobs_".into(),
            },
            BenchSection {
                name: "Latency".into(),
                headers: vec!["metric".into(), "p50".into(), "p99".into()],
                rows: vec![vec!["dispatch".into(), "12ms".into(), "45ms".into()]],
                notes: String::new(),
            },
        ];
        write_baseline_report(&path, &sections).expect("write");

        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.starts_with("# Baseline Bench Report"), "header");
        assert!(content.contains("_Generated at:"), "timestamp present");
        assert!(content.contains("## Throughput"), "section 1 title");
        assert!(content.contains("## Latency"), "section 2 title");
        assert!(
            content.contains("| workers | tasks/sec |"),
            "section 1 headers row"
        );
        assert!(content.contains("| --- | --- |"), "section 1 separator row");
        assert!(content.contains("| 1 | 9.8 |"), "section 1 first data row");
        assert!(
            content.contains("| 4 | 39.2 |"),
            "section 1 second data row"
        );
        assert!(
            content.contains("_sleep=100ms × 50 jobs_"),
            "section 1 notes"
        );
        assert!(
            content.contains("| metric | p50 | p99 |"),
            "section 2 headers"
        );
        assert!(
            content.contains("| dispatch | 12ms | 45ms |"),
            "section 2 data"
        );
        // 父目录自动建
        assert!(path.parent().expect("parent").exists(), "parent dir exists");
    }
}
