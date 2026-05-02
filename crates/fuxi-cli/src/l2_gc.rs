//! L2 ephemeral 周期 GC（Decision 21 phase 3）。
//!
//! 启动一个 tokio 长跑任务：每 `interval_secs` 秒扫一次所有注册项目的归档区，
//! 把 `archived_at < now - max_age_secs` 的 L2 归档物理删除并 publish
//! `WorkspaceCollected` 事件。
//!
//! Why 拆独立模块：im::run 已经够长——把它做成可单测的纯 async 函数（一次扫
//! 描 = `sweep_once`，长跑 loop = `run`），互不耦合。

use chrono::Duration;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::EventBus;
use fuxi_workspace::{EphemeralWorkspaceManager, FileSystemProjectRegistry};
use std::sync::Arc;
use tokio::time::{Duration as TokioDuration, interval};

/// 单次扫描——遍历 registry 列表里所有 project，调
/// `EphemeralWorkspaceManager::collect_expired(max_age)`，把回收的 task 发
/// `WorkspaceCollected`。返回总回收条目数（便于测试断言）。
pub async fn sweep_once(
    registry: &FileSystemProjectRegistry,
    bus: &EventBus,
    max_age_secs: i64,
) -> usize {
    let max_age = Duration::seconds(max_age_secs);
    let projects = match registry.list().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "L2 GC: registry.list 失败，本轮跳过");
            return 0;
        }
    };
    let mut total = 0usize;
    for project in projects {
        let mgr = EphemeralWorkspaceManager::new(project.clone(), registry.root());
        let collected = match mgr.collect_expired(max_age).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    project = %project.id,
                    error = %e,
                    "L2 GC: collect_expired 失败，跳过该项目"
                );
                continue;
            }
        };
        for (task, _meta) in collected {
            let workspace_id = fuxi_core::WorkspaceId::l2(&project.id, task);
            let mut meta = EventMeta::now();
            meta.task = Some(task);
            if let Err(e) = bus.publish(Event {
                meta,
                kind: EventKind::WorkspaceCollected { workspace_id },
            }) {
                tracing::debug!(
                    project = %project.id,
                    %task,
                    error = %e,
                    "L2 GC: publish WorkspaceCollected 失败 (silent)"
                );
            }
            total += 1;
        }
    }
    if total > 0 {
        tracing::info!(collected = total, "L2 GC sweep_once 完成");
    }
    total
}

/// 长跑 loop。`interval_secs` = 两次 sweep 间隔（默认 3600s = 1h），
/// `max_age_secs` = 归档存活上限（默认 86400s = 24h）。
///
/// 进程结束 tokio runtime 自动 abort 该 task；不持 JoinHandle。
pub async fn run(
    registry: Arc<FileSystemProjectRegistry>,
    bus: EventBus,
    interval_secs: u64,
    max_age_secs: i64,
) {
    let mut ticker = interval(TokioDuration::from_secs(interval_secs.max(1)));
    // 第一次 tick 立即触发——`tokio::time::interval` 默认 first tick = 立即。
    // 让进程启动 ~ 即扫一遍，缩短「错过昨晚 GC 窗口」场景。
    loop {
        ticker.tick().await;
        sweep_once(&registry, &bus, max_age_secs).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use futures_util::StreamExt;
    use fuxi_core::event::EventKind;
    use std::process::Command;

    fn init_repo(path: &std::path::Path) {
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .status()
            .expect("git init");
        Command::new("git")
            .args(["config", "user.email", "test@test"])
            .current_dir(path)
            .status()
            .expect("config email");
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(path)
            .status()
            .expect("config name");
        std::fs::write(path.join("README.md"), "x").expect("write");
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .expect("add");
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(path)
            .status()
            .expect("commit");
    }

    /// sweep_once 收回过期 archive 并 publish WorkspaceCollected；非过期保留。
    #[tokio::test]
    async fn sweep_once_collects_expired_and_publishes_event() {
        let dir = tempfile::tempdir().expect("tmp");
        let registry_root = dir.path().join("registry");
        let project_root = dir.path().join("erp-src");
        std::fs::create_dir_all(&project_root).expect("mkdir erp");
        init_repo(&project_root);

        let registry = FileSystemProjectRegistry::new(&registry_root);
        let project = registry
            .add(project_root.clone(), Some("erp".into()), None)
            .await
            .expect("add");

        // 起两个 ephemeral，全部 archive，把 old 那个的 archived_at 改到 2h 前
        let task_old = fuxi_core::TaskId::new();
        let task_fresh = fuxi_core::TaskId::new();

        let mgr = EphemeralWorkspaceManager::new(project.clone(), registry.root());
        mgr.create(task_old).await.expect("create old");
        mgr.archive(task_old).await.expect("archive old");
        mgr.create(task_fresh).await.expect("create fresh");
        mgr.archive(task_fresh).await.expect("archive fresh");

        let old_meta_path = registry
            .root()
            .join("erp")
            .join("archive")
            .join(task_old.to_string())
            .join("fuxi-archive-meta.json");
        let raw = std::fs::read_to_string(&old_meta_path).expect("read meta");
        let mut meta_json: serde_json::Value = serde_json::from_str(&raw).expect("parse");
        meta_json["archived_at"] =
            serde_json::Value::String((Utc::now() - Duration::hours(2)).to_rfc3339());
        std::fs::write(&old_meta_path, meta_json.to_string()).expect("write meta");

        let bus = EventBus::with_memory_store().await.expect("bus");
        let mut rx = bus.subscribe();

        let collected = sweep_once(&registry, &bus, 3_600).await;
        assert_eq!(collected, 1, "应回收 1 条过期 archive");

        // 等事件——subscribe 是 stream，用 StreamExt::next 拿。
        let mut got = None;
        for _ in 0..50 {
            match tokio::time::timeout(TokioDuration::from_millis(20), rx.next()).await {
                Ok(Some(Ok(ev))) => {
                    if let EventKind::WorkspaceCollected { workspace_id } = ev.kind
                        && workspace_id.0.contains(&task_old.0.to_string())
                    {
                        got = Some(workspace_id);
                        break;
                    }
                }
                _ => continue,
            }
        }
        assert!(got.is_some(), "WorkspaceCollected 应包含老 task uuid");

        let fresh_path = registry
            .root()
            .join("erp")
            .join("archive")
            .join(task_fresh.to_string());
        assert!(fresh_path.exists(), "fresh archive 不应被收");
    }

    /// registry 为空 → sweep_once 不发事件不报错。
    #[tokio::test]
    async fn sweep_once_handles_empty_registry() {
        let dir = tempfile::tempdir().expect("tmp");
        let registry = FileSystemProjectRegistry::new(dir.path().join("registry"));
        let bus = EventBus::with_memory_store().await.expect("bus");
        let collected = sweep_once(&registry, &bus, 3_600).await;
        assert_eq!(collected, 0);
    }
}
