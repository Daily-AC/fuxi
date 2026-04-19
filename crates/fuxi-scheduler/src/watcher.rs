//! 候吏（响应式入口）· 文件系统版：`notify` crate 监视路径。
//!
//! 设计：
//! - 每个 `fs_watch` trigger 起一条 notify `RecommendedWatcher`，命中后把事件送进 mpsc。
//! - `FsWatcherLoop::spawn` 起后台任务消费 mpsc，调 Keeper 记录+广播 `TriggerFired{cause=fs}`。
//! - debounce 由上层决定（可加 `tokio::time::sleep` 合并抖动）；v1 最简形态：每次事件都 fire。
//!
//! 公理 #3（真实时，不轮询）：`notify` 是 OS 内核级事件（FSEvents / inotify / ReadDirectoryChangesW）。

use crate::store::FireCause;
use crate::{Keeper, TriggerSpec, TriggerStore};
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// fs watcher 配置。
#[derive(Debug, Clone)]
pub struct FsWatcherConfig {
    /// 抖动合并窗口——同一 path 在此窗口内的连续事件合并为一次 fire。
    pub debounce: Duration,
}

impl Default for FsWatcherConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(500),
        }
    }
}

/// 从 store 初装所有 enabled 的 fs_watch trigger，起 watcher + 消费任务。
///
/// 返回 `JoinHandle` + `Vec<RecommendedWatcher>`——持有的 watcher 不能 drop，否则订阅取消。
pub struct FsWatcherRig {
    pub join: JoinHandle<()>,
    _watchers: Vec<RecommendedWatcher>,
}

impl FsWatcherRig {
    pub async fn spawn(
        store: TriggerStore,
        keeper: Arc<Keeper>,
        cfg: FsWatcherConfig,
    ) -> crate::Result<Self> {
        let triggers = store.list_enabled().await?;
        let mut watchers = Vec::new();
        let (tx, rx) = mpsc::unbounded_channel::<FsHit>();
        for row in triggers {
            let TriggerSpec::FsWatch { path, events } = &row.spec else {
                continue;
            };
            let trigger_id = row.id.clone();
            let path = path.clone();
            let events = events.clone();
            let tx_clone = tx.clone();
            let mut watcher =
                match notify::recommended_watcher(move |res: notify::Result<NotifyEvent>| match res
                {
                    Ok(ev) => {
                        if !should_emit(&ev, &events) {
                            return;
                        }
                        let _ = tx_clone.send(FsHit {
                            trigger_id: trigger_id.clone(),
                            kind_label: event_kind_label(&ev.kind),
                            paths: ev.paths.clone(),
                        });
                    }
                    Err(e) => warn!(error = %e, "notify error"),
                }) {
                    Ok(w) => w,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "无法建 watcher");
                        continue;
                    }
                };
            if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
                warn!(path = %path.display(), error = %e, "watch 失败");
                continue;
            }
            info!(path = %path.display(), trigger_id = %row.id, "fs watcher 已就绪");
            watchers.push(watcher);
        }
        drop(tx);
        let join = tokio::spawn(consume_loop(rx, keeper, cfg));
        Ok(Self {
            join,
            _watchers: watchers,
        })
    }
}

#[derive(Debug, Clone)]
struct FsHit {
    trigger_id: String,
    kind_label: &'static str,
    paths: Vec<PathBuf>,
}

/// 根据 trigger 的 events 白名单判定是否关心此事件。
/// `events` 空 → 全部关心。
fn should_emit(ev: &NotifyEvent, events: &[String]) -> bool {
    if events.is_empty() {
        return true;
    }
    let label = event_kind_label(&ev.kind);
    events.iter().any(|e| e == label)
}

fn event_kind_label(k: &EventKind) -> &'static str {
    match k {
        EventKind::Create(_) => "create",
        EventKind::Modify(_) => "modify",
        EventKind::Remove(_) => "remove",
        EventKind::Access(_) => "access",
        EventKind::Any => "any",
        EventKind::Other => "other",
    }
}

async fn consume_loop(
    mut rx: mpsc::UnboundedReceiver<FsHit>,
    keeper: Arc<Keeper>,
    cfg: FsWatcherConfig,
) {
    use std::collections::HashMap;
    let mut pending: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    while let Some(hit) = rx.recv().await {
        if let Some(old) = pending.remove(&hit.trigger_id) {
            old.abort();
        }
        let keeper = keeper.clone();
        let tid = hit.trigger_id.clone();
        let debounce = cfg.debounce;
        let handle = tokio::spawn(async move {
            tokio::time::sleep(debounce).await;
            let payload = serde_json::json!({
                "kind": hit.kind_label,
                "paths": hit.paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            });
            if let Err(e) = keeper
                .record_and_emit_fire(&tid, chrono::Utc::now(), FireCause::Fs, Some(payload))
                .await
            {
                warn!(trigger_id = %tid, error = %e, "fs fire 失败");
            } else {
                debug!(trigger_id = %tid, "fs fire");
            }
        });
        pending.insert(hit.trigger_id, handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keeper::SystemClock;
    use crate::spec::TriggerSpec;
    use crate::store::NewTrigger;
    use crate::{Keeper, TriggerStore, new_trigger_id};
    use futures_util::StreamExt;
    use fuxi_core::EventKind as FuxiKind;
    use fuxi_events::EventBus;
    use tempfile::tempdir;
    use tokio::fs;
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn fs_watch_fires_on_file_create() {
        let tmp = tempdir().expect("tempdir");
        let watch_path = tmp.path().to_path_buf();

        let store = TriggerStore::connect_memory().await.expect("store");
        let bus = EventBus::with_memory_store().await.expect("bus");
        let trigger_id = new_trigger_id();
        store
            .insert(NewTrigger {
                id: trigger_id.clone(),
                spec: TriggerSpec::FsWatch {
                    path: watch_path.clone(),
                    events: vec![],
                },
                intent: "监视目录".into(),
                session_id: None,
                max_failures: None,
            })
            .await
            .expect("insert");

        let keeper = Arc::new(Keeper::new(
            store.clone(),
            bus.clone(),
            Arc::new(SystemClock),
        ));
        let cfg = FsWatcherConfig {
            debounce: Duration::from_millis(50),
        };
        let rig = FsWatcherRig::spawn(store.clone(), keeper, cfg)
            .await
            .expect("spawn");

        let mut sub = bus.subscribe();

        // 制造一次写入
        tokio::time::sleep(Duration::from_millis(80)).await;
        let target = watch_path.join("ping.txt");
        fs::write(&target, b"hello").await.expect("write");

        // 等 TriggerFired cause=fs
        let mut saw = false;
        for _ in 0..20 {
            if let Ok(Some(Ok(ev))) = timeout(Duration::from_millis(500), sub.next()).await
                && let FuxiKind::TriggerFired { cause, .. } = &ev.kind
                && cause == "fs"
            {
                saw = true;
                break;
            }
        }
        // 清理
        rig.join.abort();
        assert!(saw, "期望至少一条 TriggerFired cause=fs");
        let fires = store.list_fires(&trigger_id).await.expect("fires");
        assert!(!fires.is_empty(), "DB 应有 fs fire 行");
    }

    #[test]
    fn should_emit_respects_whitelist() {
        let ev = NotifyEvent::new(EventKind::Create(notify::event::CreateKind::Any));
        assert!(should_emit(&ev, &[]), "空白名单=全放行");
        assert!(should_emit(&ev, &["create".into()]));
        assert!(!should_emit(&ev, &["modify".into()]));
    }
}
