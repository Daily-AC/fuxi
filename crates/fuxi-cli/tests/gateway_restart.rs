//! Gateway restart e2e (path 4 γ)。
//!
//! 验证 dist controller 进程死后用同一份 SQLite 重启，in-flight job 不丢。
//! α 已经做完 dual-write（enqueue/pull/report/cancel/sweep 同时写盘）+
//! `restore_from_persistence` 把 'queued'/'inflight' 行还原到 in-memory queue；
//! γ 这层 e2e 验证：跨 controller 实例 + 真 worker pump 把 5 jobs 跑到
//! ok=true 落进 dist_jobs 'done' 行。
//!
//! ## 与 α #5 / β chaos 的区别
//!
//! - α #5 (`controller_restart_via_restore_repopulates_queue`)：单元，纯 ctrl
//!   状态变更走完，验 pull 派得出去。
//! - β chaos (`tests/chaos_resilience.rs`)：worker 死掉但 controller 活，sweep
//!   重派；不涉及 controller 重启。
//! - γ 主用例：controller 死 + 真 worker pump 在新 controller 上跑完，验 SQLite
//!   终态全 'done'/ok=1——真实 gateway restart 后任务收尾闭环。
//!
//! ## 关键 fixture
//!
//! `:memory:` SQLite 库随 pool drop 一起消失，跨 controller 实例就丢了；必须用
//! tempdir 文件 path。worker 不必跨 controller 存活——drop ctrl1 时连同 worker_a
//! 一起 abort，新 ctrl2 起新 worker_c 即可。axum srv 也不复用旧 port——
//! 每次 `bind 127.0.0.1:0` 拿新 port，worker 用新 base_url。

use fuxi_cli::bench_support::spawn_one_worker;
use fuxi_cli::dist::{DistController, DistReportReq, router};
use fuxi_cli::dist_persistence::{JobPersistence, STATE_DONE};
use fuxi_events::EventBus;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// 起一个 controller(with_persistence) + axum srv 在随机端口。`:memory:` 库不能
/// 跨 controller 实例存活，参数必须是文件 path。
async fn spawn_controller_with_persistence(
    path: &Path,
) -> (Arc<DistController>, String, JoinHandle<()>) {
    let bus = EventBus::with_memory_store().await.expect("bus");
    let persistence = Arc::new(
        JobPersistence::connect_file(path)
            .await
            .expect("persistence connect_file"),
    );
    let ctrl = Arc::new(DistController::new("bench-tok".into(), bus).with_persistence(persistence));
    let app = router(ctrl.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // 与 bench_support::spawn_controller 一致：accept loop ready 的微小窗口。
    tokio::time::sleep(Duration::from_millis(20)).await;
    (ctrl, format!("http://{addr}"), handle)
}

/// 派一条无 tag、无 system_prompt 的 job，返回 job_id。
async fn enq_simple(ctrl: &DistController, title: &str) -> String {
    ctrl.enqueue(
        "gw-restart".into(),
        title.into(),
        String::new(),
        None,
        vec![],
        None,
        String::new(),
        vec![],
        None,
        None,
    )
    .await
}

/// 主用例：5 jobs / 2 worker / restart mid-flight / 验所有 5 都最终 done。
///
/// 1. tempdir SQLite 文件
/// 2. ctrl1 + 注册两个 worker（直接 ctrl1.register，跳过 axum 来回 + 真 worker
///    bring-up——本阶段只想塞 SQLite 状态，不需要真跑 job）
/// 3. enqueue 5 jobs
/// 4. ctrl1.pull("workerA") 两次（max_conc=2）+ pull("workerB") 一次
///    → SQLite 3 行 inflight + 2 行 queued
/// 5. drop ctrl1 + abort axum → 模拟 controller 进程死。SQLite 文件保留
/// 6. 起 ctrl2(same path) + restore_from_persistence
/// 7. 验 ctrl2.global_queue 含 5 个 job（α restore 把 orphan 也 push_back queue）
/// 8. 起 axum srv2 + workerC（真 worker loop, max_conc=5, sleep 50ms stub）
///    → 等 5 jobs 全 done
/// 9. 验 dist_jobs 表里 5 行 state='done', ok=1
#[tokio::test]
async fn chaos_gateway_restart_recovers_inflight_jobs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("dist.db");

    // ── ctrl1 阶段：装 inflight 状态 ──
    let (ctrl1, _base1, srv1) = spawn_controller_with_persistence(&db_path).await;
    ctrl1.register("workerA".into(), vec![], 2).await;
    ctrl1.register("workerB".into(), vec![], 1).await;

    let mut job_ids = Vec::new();
    for i in 0..5 {
        job_ids.push(enq_simple(&ctrl1, &format!("j{i}")).await);
    }

    let pull_a1 = ctrl1.pull("workerA").await.expect("workerA pull1");
    let pull_a2 = ctrl1.pull("workerA").await.expect("workerA pull2");
    let pull_b1 = ctrl1.pull("workerB").await.expect("workerB pull1");
    let inflight_set: std::collections::HashSet<String> =
        [pull_a1.id.clone(), pull_a2.id.clone(), pull_b1.id.clone()]
            .into_iter()
            .collect();
    assert_eq!(inflight_set.len(), 3, "三次 pull 应得三个不同 job");

    // SQLite 视角：drop 前必须真有 3 inflight + 2 queued，否则后面 restore 路径无意义
    {
        let p = JobPersistence::connect_file(&db_path)
            .await
            .expect("re-open persistence to inspect");
        let restored = p.restore().await.expect("restore peek");
        assert_eq!(
            restored.queued.len(),
            2,
            "drop 前 SQLite 应有 2 queued 行（pre-restart inspect）"
        );
        assert_eq!(
            restored.orphans.len(),
            3,
            "drop 前 SQLite 应有 3 inflight 行（pre-restart inspect）"
        );
    }

    // ── 模拟 controller 进程死 ──
    srv1.abort();
    drop(ctrl1);
    // 给 axum task abort + listener close 留窗口；新 ctrl 拿新 port，不会与旧 port 串。
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── ctrl2 阶段：restore + 真起 worker 跑完 ──
    let (ctrl2, base2, srv2) = spawn_controller_with_persistence(&db_path).await;
    let (queued_n, orphan_n) = ctrl2.restore_from_persistence().await;
    assert_eq!(queued_n, 2, "restore 应识别 2 queued 行");
    assert_eq!(orphan_n, 3, "restore 应识别 3 inflight 行作为 orphan");

    // workerC：真 worker loop，max_conc=5 一把吃完。bench_support::spawn_one_worker
    // 内部用 "bench-tok"，与 spawn_controller_with_persistence 一致——secret 不验签
    // （router 无 hmac_layer），token 字段对得上即可。
    let worker_handle = spawn_one_worker(
        base2.clone(),
        "workerC".into(),
        50,     // sleep_ms
        vec![], // tags
        5,      // max_concurrency
        200,    // heartbeat_ms
    );

    // 5 × 50ms 串行 = 250ms 上限，并发更快；放宽到 15s 容忍 cargo test --workspace
    // 下 ~30 个测试 binary 同时跑（多 axum + reqwest + tokio current_thread runtime
    // 抢 OS 线程）。实测原 5s 阈值在高并发整套测试时 worker register→pull 链路
    // 跑不及，all 5 stuck done=false。15s 仍远小于真 deadlock 上限。
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut all_done = true;
        for id in &job_ids {
            if !ctrl2.job_status(id).await.done {
                all_done = false;
                break;
            }
        }
        if all_done {
            break;
        }
        if Instant::now() > deadline {
            worker_handle.abort();
            srv2.abort();
            let mut snap = String::new();
            for id in &job_ids {
                let s = ctrl2.job_status(id).await;
                snap.push_str(&format!("\n  {id}: done={} ok={:?}", s.done, s.ok));
            }
            panic!("5s 内未全部 done：{snap}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // SQLite 终态断言：5 行全 state='done', ok=1
    let p = JobPersistence::connect_file(&db_path)
        .await
        .expect("final persistence inspect");
    assert_eq!(p.count().await.expect("count"), 5, "dist_jobs 总行数应 5");
    for id in &job_ids {
        let row = p
            .job_row(id)
            .await
            .expect("job_row")
            .unwrap_or_else(|| panic!("行 {id} 不存在"));
        assert_eq!(row.state, STATE_DONE, "job {id} 终态应 done");
        assert_eq!(row.ok, Some(1), "job {id} ok 应 1");
    }

    worker_handle.abort();
    srv2.abort();
}

/// 辅助 #1：dist_jobs 里有 'done' 行时，restore 不重新 enqueue 它们。
/// 验"重启不重跑已完成 job"——纯通过 ctrl 状态变更走完路径，不起 worker。
#[tokio::test]
async fn restore_from_persistence_skips_done_jobs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("dist.db");

    // ctrl1：enqueue 3 → pull 2 → report 1 ok → 1 done + 1 inflight + 1 queued
    let (ctrl1, _base1, srv1) = spawn_controller_with_persistence(&db_path).await;
    ctrl1.register("workerA".into(), vec![], 2).await;
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(enq_simple(&ctrl1, &format!("j{i}")).await);
    }
    let p1 = ctrl1.pull("workerA").await.expect("pull1");
    let p2 = ctrl1.pull("workerA").await.expect("pull2");
    let report_ok = ctrl1
        .report(DistReportReq {
            node_id: "workerA".into(),
            job_id: p1.id.clone(),
            ok: true,
            output: "done".into(),
            duration_ms: 10,
        })
        .await;
    assert!(report_ok, "report 应被 controller 接受");

    srv1.abort();
    drop(ctrl1);
    tokio::time::sleep(Duration::from_millis(20)).await;

    // ctrl2：restore 应只看到 1 queued + 1 orphan = 2，不含 done 那个
    let (ctrl2, _base2, srv2) = spawn_controller_with_persistence(&db_path).await;
    let (queued_n, orphan_n) = ctrl2.restore_from_persistence().await;
    assert_eq!(queued_n, 1, "未 pull 的 1 个应作为 queued 还原");
    assert_eq!(orphan_n, 1, "inflight 的 1 个应作为 orphan 还原");
    assert_eq!(queued_n + orphan_n, 2, "已 done 的 p1 不应再被 enqueue");

    // 已 done 行仍在表里（审计语义）
    let p = JobPersistence::connect_file(&db_path)
        .await
        .expect("inspect");
    let done_row = p
        .job_row(&p1.id)
        .await
        .expect("done_row")
        .expect("done 行应保留作为审计");
    assert_eq!(done_row.state, STATE_DONE);
    assert_eq!(done_row.ok, Some(1));

    // ctrl2 也应看不到 p1 在 queue 里——通过 pull 验证：能 pull 出 2 个但都不是 p1
    ctrl2.register("workerB".into(), vec![], 5).await;
    let mut pulled_ids = Vec::new();
    while let Some(job) = ctrl2.pull("workerB").await {
        pulled_ids.push(job.id);
    }
    assert_eq!(pulled_ids.len(), 2, "重启后只应能 pull 出 2 个未完成 job");
    assert!(
        !pulled_ids.contains(&p1.id),
        "已 done 的 p1 不应再被 pull 出去"
    );
    assert!(
        pulled_ids.contains(&p2.id),
        "原 inflight 的 p2 应作为 orphan 重派"
    );
    let third = ids
        .iter()
        .find(|id| **id != p1.id && **id != p2.id)
        .expect("third");
    assert!(pulled_ids.contains(third), "未 pull 的 queued job 应在");

    srv2.abort();
}

/// 辅助 #2：5 个 queued 按 enqueued_at 顺序还原 queue 顺序。
/// 验"重启不重排"——FIFO 派工次序穿越 restart 仍稳定。
#[tokio::test]
async fn restore_from_persistence_preserves_enqueue_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join("dist.db");

    let (ctrl1, _base1, srv1) = spawn_controller_with_persistence(&db_path).await;
    let mut ids = Vec::new();
    for i in 0..5 {
        let id = enq_simple(&ctrl1, &format!("ord-{i}")).await;
        ids.push(id);
        // sleep 2ms 让 enqueued_at 的 RFC3339 字符串严格单调——同毫秒内
        // ORDER BY enqueued_at, id 的 id 二级 tie-break 让结果不可预测；
        // 2ms 足够避开 chrono 的 ms-resolution 字符串截断。
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    srv1.abort();
    drop(ctrl1);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (ctrl2, _base2, srv2) = spawn_controller_with_persistence(&db_path).await;
    let (queued_n, orphan_n) = ctrl2.restore_from_persistence().await;
    assert_eq!(queued_n, 5);
    assert_eq!(orphan_n, 0);

    // 通过 pull 验证 queue 顺序——integration test 拿不到 inner 锁，pull 是
    // 唯一观察 queue head 的公共 API。worker 注册 max_conc=5 一次 pull 5 个，
    // 顺序应严格匹配 enqueue 顺序。
    ctrl2.register("workerB".into(), vec![], 5).await;
    let mut pulled_order = Vec::new();
    while let Some(job) = ctrl2.pull("workerB").await {
        pulled_order.push(job.id);
    }
    assert_eq!(pulled_order.len(), 5);
    assert_eq!(
        pulled_order, ids,
        "restore 后 queue 顺序应严格匹配 enqueue 顺序"
    );

    srv2.abort();
}
