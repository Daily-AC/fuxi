//! Chaos resilience suite (path 4 β)。
//!
//! 验证 dist worker 在不同失败姿态下"已派出 / 排队的 job 不丢"——
//! kill -9 mid-job、网络分区短/长、并发多 worker 随机 kill。
//!
//! ## 时序设计两条铁律
//!
//! 1. **不真等 sweep 60s**——直接调 `sweep_stale(now, Duration::from_millis(N))`
//!    把 stale 阈值缩到 N 毫秒；worker abort 后心跳停了，几百 ms 内 last_seen
//!    就老化达标。比手动 mutate `inner.last_seen` 干净，且不依赖 controller 私
//!    有字段。
//! 2. **abort worker JoinHandle = 模拟 OS-level kill -9**——tokio task abort
//!    不跑 drop，与真 SIGKILL 同语义（不发 final report，process cleanup 不走）。
//!
//! ## 已知功能 gap（chaos #3 揭示）
//!
//! `chaos_partition_exceeds_sweep_timeout_worker_re_registers` 验"partition >
//! sweep timeout 后 worker 能否复活"。当前实现下：
//! - controller 端 `heartbeat` 用 `entry().or_default()` 静默重建 node entry，
//!   恢复后心跳第一拍把 worker 视图刷回 alive。
//! - **副作用**：重建出的 entry 用默认值（`tags=[]` + `max_concurrency=1`），
//!   原 register 声明的 tags 丢失。本测试用 untagged job 验证基本通路；带 tag
//!   的 job 在 sweep-then-recover 路径下会**派不出去**——已 flag 给 lead 作
//!   path 4 follow-up。

use fuxi_cli::bench_support::{
    spawn_controller, spawn_one_worker, spawn_one_worker_shared_active, spawn_partition_proxy,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// 派一条无 tag、无 system_prompt 的 job，返回 job_id。
async fn enq_simple(ctrl: &fuxi_cli::dist::DistController, title: &str) -> String {
    ctrl.enqueue(
        "chaos".into(),
        title.into(),
        String::new(),
        None,
        vec![],
        None,
        String::new(),
        vec![],
    )
    .await
}

/// chaos 1：workerA mid-job 被 kill -9，sweep_stale 把 orphan 推回 queue，
/// workerB 拉走完成。
///
/// **真 kill -9 的等价模拟**：tokio JoinHandle::abort 只 cancel 外层 task，但
/// `run_worker_with` 内部 `tokio::spawn` 出去的心跳 task 是独立 task，外层 abort
/// 不传染 → 心跳继续报，last_seen 永不老化，sweep 看不到 orphan。**真 SIGKILL
/// 杀整个进程，所有 task 一并消失**。这里用"abort outer + 通过 partition proxy
/// 阻断网络"等价模拟"进程死透"——心跳即便还在转，包也出不去 controller。
#[tokio::test]
async fn chaos_kill_9_worker_midjob_redispatches_to_healthy_worker() {
    let (ctrl, base, srv) = spawn_controller().await;
    let (proxy_url, partition_a, proxy_handle) = spawn_partition_proxy(base.clone()).await;

    // workerA 走 proxy；workerB 直连 controller，不受 proxy 影响
    let worker_a = spawn_one_worker(
        proxy_url.clone(),
        "killA".into(),
        10_000, // 10s sleep job
        vec![],
        1,
        100,
    );
    let job_id = enq_simple(&ctrl, "kill-victim").await;

    // 等 A pickup（job 已进 controller.inflight + node.inflight）
    let pickup_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let info = ctrl.node_info("killA").await;
        if info.as_ref().map(|i| i.inflight.len()) == Some(1) {
            break;
        }
        if Instant::now() > pickup_deadline {
            worker_a.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("workerA inflight 3s 内未同步给 controller");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 模拟 kill -9：abort outer task + partition 阻断所有出包
    worker_a.abort();
    partition_a.store(true, std::sync::atomic::Ordering::SeqCst);
    // 等心跳被吞 + last_seen 老化
    tokio::time::sleep(Duration::from_millis(500)).await;

    // sweep 用短阈值（300ms）——hb 全 503，controller 端 last_seen 已老化达标
    let recycled = ctrl
        .sweep_stale(Instant::now(), Duration::from_millis(300))
        .await;
    let killa_recycled: Vec<_> = recycled
        .iter()
        .filter(|(n, _)| n == "killA")
        .cloned()
        .collect();
    assert_eq!(killa_recycled.len(), 1, "应回收 killA, 实际 {recycled:?}");
    assert_eq!(killa_recycled[0].1, vec![job_id.clone()]);

    // 起 workerB 直连真 controller（绕过 proxy），应拉到 job 完成
    let worker_b = spawn_one_worker(base.clone(), "healB".into(), 80, vec![], 1, 100);

    let deadline = Instant::now() + Duration::from_secs(5);
    let final_status = loop {
        let s = ctrl.job_status(&job_id).await;
        if s.done {
            break s;
        }
        if Instant::now() > deadline {
            worker_b.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("workerB 5s 内未完成被回收的 job");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(final_status.ok, Some(true), "重派后应 ok=true");
    assert_eq!(
        final_status.node_id.as_deref(),
        Some("healB"),
        "终态 node_id 应是 healB（kill 掉的 killA 不应是 assignee）"
    );

    worker_b.abort();
    proxy_handle.abort();
    srv.abort();
}

/// chaos 2：worker 跑 job 时链路被全断 ~600ms（< sweep timeout），
/// 链路恢复后 worker 自动续传 progress / heartbeat 并完成 job。
/// 验证 inflight 视图最终对账 0 + 没被误 sweep。
#[tokio::test]
async fn chaos_network_partition_then_recover_keeps_inflight_consistent() {
    let (ctrl, base, srv) = spawn_controller().await;
    let (proxy_url, partition, proxy_handle) = spawn_partition_proxy(base.clone()).await;

    let worker = spawn_one_worker(
        proxy_url.clone(),
        "partN".into(),
        900, // job 跑 900ms，覆盖 partition 期间 + 恢复后
        vec![],
        1,
        100,
    );

    // 等 worker register
    let reg_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if ctrl.node_info("partN").await.is_some() {
            break;
        }
        if Instant::now() > reg_deadline {
            worker.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("worker 3s 内未 register（proxy 路由透传出问题）");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let job_id = enq_simple(&ctrl, "partition-job").await;
    // 等 pickup（node.inflight 长度变 1）
    let pickup_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let info = ctrl.node_info("partN").await;
        if info.as_ref().map(|i| i.inflight.len()) == Some(1) {
            break;
        }
        if Instant::now() > pickup_deadline {
            worker.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("worker 3s 内未 pickup partition-job");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let info_before = ctrl.node_info("partN").await.expect("partN registered");
    let last_seen_before = info_before.last_seen.expect("有 last_seen");

    // 链路全断 600ms（约 6 个心跳间隔）
    partition.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(600)).await;
    partition.store(false, std::sync::atomic::Ordering::SeqCst);

    // 期待：恢复后 hb 推 last_seen + report 落地 + inflight 收敛 0
    let deadline = Instant::now() + Duration::from_secs(5);
    let final_status = loop {
        let s = ctrl.job_status(&job_id).await;
        if s.done {
            break s;
        }
        if Instant::now() > deadline {
            worker.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("partition 恢复后 5s 内 job 未完成（report 重试路径破了）");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(final_status.ok, Some(true), "partition 恢复后应 ok=true");

    // 给恢复后的心跳一拍刷新
    tokio::time::sleep(Duration::from_millis(300)).await;
    let info_post = ctrl.node_info("partN").await.expect("worker 仍在");
    let last_seen_post = info_post.last_seen.expect("post hb last_seen");
    assert!(
        last_seen_post > last_seen_before,
        "恢复后 last_seen 应推进, before={last_seen_before:?} post={last_seen_post:?}"
    );
    assert!(
        info_post.inflight.is_empty(),
        "终态 worker.inflight 应清空, 实际: {:?}",
        info_post.inflight
    );

    // 没被误 sweep——partition < 阈值
    let recycled = ctrl
        .sweep_stale(Instant::now(), Duration::from_secs(30))
        .await;
    assert!(
        recycled.is_empty(),
        "partition < 阈值不应触发 sweep, 实际: {recycled:?}"
    );

    worker.abort();
    proxy_handle.abort();
    srv.abort();
}

/// chaos 3：partition 持续超过 sweep 阈值，partition 中 controller 主动 sweep，
/// 链路恢复后 worker 应能继续工作（接新 job）。
///
/// **见模块 doc**：当前 worker 不主动检测自己被 sweep，靠 controller heartbeat
/// 的 `entry().or_default()` 自愈，**会丢 register 时的 tags**。本测试用
/// untagged job 验基本通路。
#[tokio::test]
async fn chaos_partition_exceeds_sweep_timeout_worker_re_registers() {
    let (ctrl, base, srv) = spawn_controller().await;
    let (proxy_url, partition, proxy_handle) = spawn_partition_proxy(base.clone()).await;

    // 声明 ["t1"] tag——sweep 后 entry 重建会丢失，本测试**不**断言 tag 保留
    // （那是 follow-up 真功能 gap）。
    let worker = spawn_one_worker(
        proxy_url.clone(),
        "rejN".into(),
        80,
        vec!["t1".into()],
        1,
        100,
    );

    // 等 register（带 tag）
    let reg_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(info) = ctrl.node_info("rejN").await
            && info.tags == vec!["t1".to_string()]
        {
            break;
        }
        if Instant::now() > reg_deadline {
            worker.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("worker 3s 内未注册成功（带 t1 tag）");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // partition 长时段
    partition.store(true, std::sync::atomic::Ordering::SeqCst);
    // 等心跳被拒 + last_seen 老化达 sweep 阈值
    tokio::time::sleep(Duration::from_millis(500)).await;

    // partition 中 controller 主动 sweep（短阈值 300ms 让 last_seen 已老化达标）
    let _swept = ctrl
        .sweep_stale(Instant::now(), Duration::from_millis(300))
        .await;
    // sweep 不删 node entry——只清空 inflight（partition 期间 worker 没 inflight,
    // 主要是 last_seen 标记 stale）。

    // 恢复
    partition.store(false, std::sync::atomic::Ordering::SeqCst);

    // enqueue untagged job——sweep 后 entry 即便被默认值重建，untagged job 仍
    // 能匹配（`required_tags ⊆ worker_tags`，空集是任何集合的子集）
    let job_untagged = enq_simple(&ctrl, "after-sweep-untagged").await;
    let deadline = Instant::now() + Duration::from_secs(5);
    let final_status = loop {
        let s = ctrl.job_status(&job_untagged).await;
        if s.done {
            break s;
        }
        if Instant::now() > deadline {
            worker.abort();
            proxy_handle.abort();
            srv.abort();
            panic!("sweep 后 worker 5s 内未拉新 untagged job");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(final_status.ok, Some(true));
    assert_eq!(final_status.node_id.as_deref(), Some("rejN"));

    // 验证恢复后 worker 仍存在 + last_seen 已被新 hb 推进
    let info_post = ctrl.node_info("rejN").await.expect("worker 仍存在");
    let last_seen_post = info_post.last_seen.expect("post hb last_seen");
    let now = Instant::now();
    assert!(
        now.saturating_duration_since(last_seen_post) < Duration::from_secs(5),
        "恢复后 last_seen 应近期, 距今 {:?}",
        now.saturating_duration_since(last_seen_post)
    );

    worker.abort();
    proxy_handle.abort();
    srv.abort();
}

/// chaos 4：4 worker × 20 job，期间 kill 2 个 worker（abort + 切 partition），
/// sweep 重派，验证 20 个 job 全部完成、无丢失。
///
/// **kill -9 等价模拟**：见 chaos #1 注释。w0/w1 各走自己的 partition proxy，
/// kill 时 abort outer + partition=true → 心跳即便还在转，包出不去。
#[tokio::test]
async fn chaos_concurrent_4_workers_random_kill_redispatch_no_loss() {
    let (ctrl, base, srv) = spawn_controller().await;

    // 给 w0/w1 各自一个 proxy（kill 时切 partition）。w2/w3 直连真 controller。
    let (proxy_w0, partition_w0, proxy_w0_handle) = spawn_partition_proxy(base.clone()).await;
    let (proxy_w1, partition_w1, proxy_w1_handle) = spawn_partition_proxy(base.clone()).await;

    let active = Arc::new(AtomicUsize::new(0));
    // killable workers 走各自 proxy；w2/w3 直连真 controller
    let workers: Vec<_> = vec![
        spawn_one_worker_shared_active(proxy_w0.clone(), "w0".into(), 200, active.clone(), 100),
        spawn_one_worker_shared_active(proxy_w1.clone(), "w1".into(), 200, active.clone(), 100),
        spawn_one_worker_shared_active(base.clone(), "w2".into(), 200, active.clone(), 100),
        spawn_one_worker_shared_active(base.clone(), "w3".into(), 200, active.clone(), 100),
    ];

    let cleanup = |workers: &Vec<tokio::task::JoinHandle<()>>| {
        for w in workers {
            w.abort();
        }
        proxy_w0_handle.abort();
        proxy_w1_handle.abort();
        srv.abort();
    };

    // 等 4 worker 全注册
    let reg_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snap = ctrl.nodes_snapshot().await;
        if snap.len() == 4 {
            break;
        }
        if Instant::now() > reg_deadline {
            cleanup(&workers);
            panic!("4 worker 3s 内未全注册（实际 {} 个）", snap.len());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // enqueue 20 job
    let mut job_ids = Vec::new();
    for i in 0..20 {
        job_ids.push(enq_simple(&ctrl, &format!("c4-{i}")).await);
    }

    // 让派工先跑起来——active>=2 后再 kill，确保 abort 真打中 in-flight
    let pickup_deadline = Instant::now() + Duration::from_secs(3);
    while active.load(Ordering::SeqCst) < 2 {
        if Instant::now() > pickup_deadline {
            cleanup(&workers);
            panic!("3s 内 active worker 未到 2 个");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // 杀前两个 worker（确定性"随机"——固定 index 让 flake 可复现，
    // 真随机会让 CI 抖到没法 bisect）
    workers[0].abort();
    workers[1].abort();
    partition_w0.store(true, std::sync::atomic::Ordering::SeqCst);
    partition_w1.store(true, std::sync::atomic::Ordering::SeqCst);

    // 等心跳被吞 + last_seen 老化
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 等所有 20 job 完成
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut done_count;
    let mut last_sweep = Instant::now() - Duration::from_secs(1); // 立即触发首轮
    loop {
        done_count = 0;
        for jid in &job_ids {
            if ctrl.job_status(jid).await.done {
                done_count += 1;
            }
        }
        if done_count == 20 {
            break;
        }
        // 每秒主动 sweep 一次——chaos #4 的关键不变量是"任何被 kill 的
        // worker 抢到的 job 终将被回收"，定期 sweep 模拟 controller 端
        // spawn_sweep_task 的 30s tick 但缩到 1s 加快测试。
        if last_sweep.elapsed() >= Duration::from_secs(1) {
            let _ = ctrl
                .sweep_stale(Instant::now(), Duration::from_millis(300))
                .await;
            last_sweep = Instant::now();
        }
        if Instant::now() > deadline {
            cleanup(&workers);
            panic!("15s 内仅 {done_count}/20 job 完成（有丢失）");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // 全部 ok=true（无 worker 报错）+ assignee ∈ {w2,w3}
    for jid in &job_ids {
        let s = ctrl.job_status(jid).await;
        assert!(s.done, "{jid} 应 done");
        assert_eq!(s.ok, Some(true), "{jid} 应 ok=true");
        // 终态 assignee 应是 w2/w3 之一——被 kill 的 w0/w1 即便先 pickup 也
        // 不会发出 final report，controller.finished 里的 node_id 来自实际完成方
        let assignee = s.node_id.as_deref().unwrap_or("");
        assert!(
            assignee == "w2" || assignee == "w3",
            "{jid} assignee 应 ∈ {{w2,w3}}, 实际 {assignee}"
        );
    }

    cleanup(&workers);
}
