//! fuxi daemon——Unix socket 上的玄女控制面。
//!
//! 职责：
//! 1. 听 `$FUXI_SOCK`（默认 `/tmp/fuxi.sock`）
//! 2. 每来一个 client 连接起一个 task，按 [`ipc::Command`] 派发到 `Fuxi`
//! 3. 写回 [`ipc::Response`]
//!
//! 不负责：
//! - 事件流转发（那是 firehose Hub 的事，走 WS/SSE）
//! - REPL UI（那是薄片 D）
//! - 玄女 CC 实例的生命周期（daemon 启动时由 caller spawn 玄女 + 往这
//!   个 socket 塞 `FUXI_SOCK` 给她）

use crate::ipc::{Command, InterveneMode, Response};
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::AgentId;
use fuxi_core::task::Task;
use fuxi_events::EventBus;
use fuxi_orchestrator::{Fuxi, WorkerKind};
use fuxi_scheduler::store::{FireCause, NewTrigger};
use fuxi_scheduler::{Keeper, TriggerSpec, TriggerStore, new_trigger_id};
use fuxi_skills as skill_loader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use uuid::Uuid;

/// daemon 运行所需的所有状态。
pub struct Daemon {
    pub fuxi: Arc<Fuxi>,
    /// EventBus——给 `TriggerRegistered` 广播用（daemon 不经 Fuxi 直发）。
    pub bus: EventBus,
    /// 更漏候簿——trigger CRUD 直接落库。Keeper 下一 tick 自动可见。
    pub store: TriggerStore,
    /// Keeper 句柄——给"手动 fire"路径用，和 cron tick 共用 `record_and_emit_fire`。
    pub keeper: Arc<Keeper>,
    /// 发信号给 serve 循环的 abort——Command::Shutdown 触发。
    shutdown_signal: Arc<Notify>,
}

impl Daemon {
    pub fn new(fuxi: Arc<Fuxi>, bus: EventBus, store: TriggerStore, keeper: Arc<Keeper>) -> Self {
        Self {
            fuxi,
            bus,
            store,
            keeper,
            shutdown_signal: Arc::new(Notify::new()),
        }
    }

    /// 阻塞到收到 Shutdown 命令或 serve 循环错误。
    pub async fn serve(self, socket_path: &Path) -> Result<()> {
        // 清理残留 socket——daemon 异常崩后重启会踩到 "Address already in use"
        if socket_path.exists() {
            std::fs::remove_file(socket_path)
                .with_context(|| format!("清理残留 socket {}", socket_path.display()))?;
        }
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let listener = UnixListener::bind(socket_path)
            .with_context(|| format!("bind {}", socket_path.display()))?;
        tracing::info!(path = %socket_path.display(), "daemon 监听中");

        let notify = self.shutdown_signal.clone();
        let fuxi = self.fuxi.clone();
        let shutdown_hook = self.shutdown_signal.clone();
        let store = self.store.clone();
        let keeper = self.keeper.clone();
        let bus = self.bus.clone();

        loop {
            tokio::select! {
                _ = notify.notified() => {
                    tracing::info!("daemon 收到 Shutdown 信号，停止 accept");
                    break;
                }
                res = listener.accept() => {
                    let (stream, _) = res.context("accept")?;
                    let fuxi = fuxi.clone();
                    let store = store.clone();
                    let keeper = keeper.clone();
                    let bus = bus.clone();
                    let shutdown_hook = shutdown_hook.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, fuxi, bus, store, keeper, shutdown_hook).await {
                            tracing::warn!(error = %e, "connection handler errored");
                        }
                    });
                }
            }
        }

        let _ = std::fs::remove_file(socket_path);
        Ok(())
    }

    /// 用来给 `Ctrl-C` handler 外部触发 shutdown。
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        self.shutdown_signal.clone()
    }
}

/// 解析一行 JSON 命令 → 派发 → 写回一行响应。每条连接只处理一条命令，然后断开。
async fn handle_conn(
    stream: UnixStream,
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    store: TriggerStore,
    keeper: Arc<Keeper>,
    shutdown_hook: Arc<Notify>,
) -> Result<()> {
    let (rx, mut tx) = stream.into_split();
    let mut reader = BufReader::new(rx).lines();

    // 只读一行——JSON 单条命令
    let Some(line) = reader.next_line().await? else {
        return Ok(());
    };
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    let resp = match serde_json::from_str::<Command>(trimmed) {
        Ok(cmd) => dispatch_command(fuxi, bus, store, keeper, cmd, shutdown_hook).await,
        Err(e) => Response::err(format!("解析命令失败: {e}")),
    };
    let out = serde_json::to_string(&resp)? + "\n";
    tx.write_all(out.as_bytes()).await?;
    tx.flush().await?;
    Ok(())
}

async fn dispatch_command(
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    store: TriggerStore,
    keeper: Arc<Keeper>,
    cmd: Command,
    shutdown_hook: Arc<Notify>,
) -> Response {
    match cmd {
        Command::Ping => Response::Pong,

        Command::Spawn { role, name } => match spawn_by_role(&fuxi, &role, name).await {
            Ok(id) => Response::ok(serde_json::json!({"agent_id": id.to_string()})),
            Err(e) => Response::err(e.to_string()),
        },

        Command::Dispatch {
            agent_id,
            title,
            body,
        } => match parse_agent_id(&agent_id) {
            Err(e) => Response::err(e),
            Ok(id) => {
                let task = Task::new(&title, body.as_deref().unwrap_or(""));
                let task_id = task.id;
                match fuxi.dispatch(id, task).await {
                    Ok(()) => Response::ok(serde_json::json!({"task_id": task_id.to_string()})),
                    Err(e) => Response::err(e.to_string()),
                }
            }
        },

        Command::Intervene {
            agent_id,
            mode,
            text,
        } => match parse_agent_id(&agent_id) {
            Err(e) => Response::err(e),
            Ok(id) => {
                let interrupt_first = matches!(mode, InterveneMode::Interrupt);
                match fuxi.intervene(id, interrupt_first, &text).await {
                    Ok(()) => Response::ok(serde_json::json!({"delivered": true})),
                    Err(e) => Response::err(e.to_string()),
                }
            }
        },

        Command::Status { agent_id } => match agent_id {
            Some(raw) => match parse_agent_id(&raw) {
                Err(e) => Response::err(e),
                Ok(id) => match fuxi.status_of(id).await {
                    Some(s) => Response::ok(serde_json::json!({"status": format!("{s:?}")})),
                    None => Response::err(format!("agent {raw} 不在 shelf")),
                },
            },
            None => {
                let n = fuxi.worker_count().await;
                Response::ok(serde_json::json!({"worker_count": n}))
            }
        },

        Command::List => {
            let cards = fuxi.list_workers().await;
            let items: Vec<_> = cards
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id.to_string(),
                        "name": c.profile.name,
                        "role": c.profile.role,
                        "cli": c.profile.cli,
                        "endpoint": c.endpoint,
                        "status": format!("{:?}", c.status),
                    })
                })
                .collect();
            Response::ok(serde_json::json!({"workers": items}))
        }

        Command::Kill { agent_id } => match parse_agent_id(&agent_id) {
            Err(e) => Response::err(e),
            Ok(_id) => {
                // v0.1 简化：整个 shutdown 不够精细，但没有"只 kill 一个门客"的
                // 现成 API。薄片 I/D 会补 `Fuxi::kill_worker`。当前返回提示。
                Response::err("Kill-one 暂未实装；用 Shutdown 关全部".to_string())
            }
        },

        Command::BlockTask { task_id, reason } => match parse_task_id(&task_id) {
            Err(e) => Response::err(e),
            Ok(tid) => match fuxi.block_task(tid, reason) {
                Ok(()) => Response::ok(serde_json::json!({"blocked": true})),
                Err(e) => Response::err(e.to_string()),
            },
        },

        Command::ResumeTask { task_id, input } => match parse_task_id(&task_id) {
            Err(e) => Response::err(e),
            Ok(tid) => match fuxi.resume_task(tid, input) {
                Ok(()) => Response::ok(serde_json::json!({"resumed": true})),
                Err(e) => Response::err(e.to_string()),
            },
        },

        Command::EmitEvent { kind } => {
            use fuxi_core::{Event, EventMeta};
            let ev = Event {
                meta: EventMeta::now(),
                kind: kind.into_event_kind(),
            };
            match fuxi.bus().publish(ev) {
                Ok(_) => Response::ok(serde_json::json!({"emitted": true})),
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::Shutdown => {
            // 先触发 Fuxi 的门客收尾，再通知 serve 循环退出
            if let Err(e) = fuxi.shutdown().await {
                tracing::warn!(error = %e, "fuxi.shutdown 部分失败，继续停 daemon");
            }
            shutdown_hook.notify_waiters();
            Response::ok(serde_json::json!({"shutdown": true}))
        }

        // ── 更漏 ──
        Command::CronAdd {
            expr,
            intent,
            tz,
            session_id,
        } => {
            let id = new_trigger_id();
            let spec = TriggerSpec::Cron { expr, tz };
            match store
                .insert(NewTrigger {
                    id: id.clone(),
                    spec: spec.clone(),
                    intent,
                    session_id,
                    max_failures: None,
                })
                .await
            {
                Ok(row) => {
                    emit_trigger_registered(&bus, &row.id, &row.spec);
                    Response::ok(serde_json::json!({"trigger_id": row.id, "kind": "cron"}))
                }
                Err(e) => Response::err(e.to_string()),
            }
        }
        Command::CronOnce {
            at,
            intent,
            session_id,
        } => match DateTime::parse_from_rfc3339(&at) {
            Err(e) => Response::err(format!("at 需 RFC3339: {e}")),
            Ok(at) => {
                let id = new_trigger_id();
                let spec = TriggerSpec::Once {
                    at: at.with_timezone(&Utc),
                };
                match store
                    .insert(NewTrigger {
                        id: id.clone(),
                        spec: spec.clone(),
                        intent,
                        session_id,
                        max_failures: None,
                    })
                    .await
                {
                    Ok(row) => {
                        emit_trigger_registered(&bus, &row.id, &row.spec);
                        Response::ok(serde_json::json!({"trigger_id": row.id, "kind": "once"}))
                    }
                    Err(e) => Response::err(e.to_string()),
                }
            }
        },
        Command::CronWatch {
            path,
            intent,
            events,
            session_id,
        } => {
            let spec = TriggerSpec::FsWatch {
                path: PathBuf::from(path),
                events,
            };
            let id = new_trigger_id();
            match store
                .insert(NewTrigger {
                    id: id.clone(),
                    spec: spec.clone(),
                    intent,
                    session_id,
                    max_failures: None,
                })
                .await
            {
                Ok(row) => {
                    emit_trigger_registered(&bus, &row.id, &row.spec);
                    // v1：fs_watch 只在 fuxi up 启动时批量挂载。运行时新增暂留提示。
                    Response::ok(serde_json::json!({
                        "trigger_id": row.id,
                        "kind": "fs_watch",
                        "note": "fs 监视在下次 fuxi up 启动时生效",
                    }))
                }
                Err(e) => Response::err(e.to_string()),
            }
        }
        Command::CronWebhook {
            intent,
            secret,
            session_id,
        } => {
            let spec = TriggerSpec::Webhook { secret };
            let id = new_trigger_id();
            match store
                .insert(NewTrigger {
                    id: id.clone(),
                    spec: spec.clone(),
                    intent,
                    session_id,
                    max_failures: None,
                })
                .await
            {
                Ok(row) => {
                    emit_trigger_registered(&bus, &row.id, &row.spec);
                    Response::ok(serde_json::json!({
                        "trigger_id": row.id,
                        "kind": "webhook",
                        "endpoint": format!("POST /hook/{}", row.id),
                    }))
                }
                Err(e) => Response::err(e.to_string()),
            }
        }
        Command::CronList => match store.list_all().await {
            Ok(rows) => {
                let items: Vec<_> = rows
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.id,
                            "kind": r.spec.kind_str(),
                            "intent": r.intent,
                            "enabled": r.enabled,
                            "consecutive_failures": r.consecutive_failures,
                            "last_fired_at": r.last_fired_at.map(|d| d.to_rfc3339()),
                            "spec": r.spec,
                        })
                    })
                    .collect();
                Response::ok(serde_json::json!({"triggers": items}))
            }
            Err(e) => Response::err(e.to_string()),
        },
        Command::CronFire { id } => match store.get(&id).await {
            Ok(Some(row)) => {
                if !row.enabled {
                    Response::err(format!("trigger {id} 已 disabled"))
                } else {
                    match keeper
                        .record_and_emit_fire(&id, Utc::now(), FireCause::Manual, None)
                        .await
                    {
                        Ok(_) => Response::ok(serde_json::json!({"fired": true, "trigger_id": id})),
                        Err(e) => Response::err(e.to_string()),
                    }
                }
            }
            Ok(None) => Response::err(format!("trigger {id} 不存在")),
            Err(e) => Response::err(e.to_string()),
        },
        Command::CronRemove { id } => match store.remove(&id).await {
            Ok(true) => Response::ok(serde_json::json!({"removed": true})),
            Ok(false) => Response::err(format!("trigger {id} 不存在")),
            Err(e) => Response::err(e.to_string()),
        },
    }
}

/// 登记成功后发一条 `TriggerRegistered` 事件——Firehose 能看到候簿变化。
fn emit_trigger_registered(bus: &EventBus, id: &str, spec: &TriggerSpec) {
    let spec_json = serde_json::to_value(spec).unwrap_or(serde_json::Value::Null);
    let ev = Event {
        meta: EventMeta::now(),
        kind: EventKind::TriggerRegistered {
            id: id.to_string(),
            kind: spec.kind_str().to_string(),
            spec: spec_json,
        },
    };
    let _ = bus.publish(ev);
}

fn parse_agent_id(s: &str) -> std::result::Result<AgentId, String> {
    // AgentId 的 Display 是 "agent-<uuid>"；解析时去掉前缀。容错：纯 UUID 也接收。
    let uuid_part = s.strip_prefix("agent-").unwrap_or(s);
    Uuid::parse_str(uuid_part)
        .map(AgentId::from)
        .map_err(|e| format!("无效 agent_id {s:?}: {e}"))
}

fn parse_task_id(s: &str) -> std::result::Result<fuxi_core::id::TaskId, String> {
    let uuid_part = s.strip_prefix("task-").unwrap_or(s);
    Uuid::parse_str(uuid_part)
        .map(fuxi_core::id::TaskId::from)
        .map_err(|e| format!("无效 task_id {s:?}: {e}"))
}

/// 根据 role 读 Skill → 构造 AgentProfile + CcLaunchConfig → 丢给 Fuxi.spawn_worker
async fn spawn_by_role(fuxi: &Fuxi, role: &str, name_override: Option<String>) -> Result<AgentId> {
    let loaded =
        skill_loader::load(role).with_context(|| format!("加载 skills/{role}/SKILL.md"))?;
    let mut profile = loaded.profile;
    if let Some(n) = name_override {
        profile.name = n;
    }
    let cfg = CcLaunchConfig {
        append_system_prompt: if loaded.append_system_prompt.is_empty() {
            None
        } else {
            Some(loaded.append_system_prompt)
        },
        allowed_tools: loaded.allowed_tools,
        ..Default::default()
    };
    fuxi.spawn_worker(profile, WorkerKind::Cc(cfg))
        .await
        .map_err(|e| anyhow!(e.to_string()))
}

/// 供测试：构造一个 socket 路径在临时目录下。
#[cfg(test)]
#[allow(dead_code)]
fn temp_sock_path() -> PathBuf {
    tempfile::NamedTempFile::new()
        .unwrap()
        .into_temp_path()
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Command;

    /// 不起真 Fuxi——用空壳子验 wire 行为（ping/invalid cmd）。
    async fn mock_daemon_parts() -> (Arc<Fuxi>, EventBus, TriggerStore, Arc<Keeper>) {
        use fuxi_orchestrator::FuxiConfig;
        use fuxi_scheduler::keeper::SystemClock;
        use fuxi_workspace::GitWorktreeWorkspace;
        let bus = EventBus::with_memory_store().await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            tmp.path().to_path_buf(),
        ));
        let cfg = FuxiConfig {
            allocate_worktree: false,
            ..Default::default()
        };
        let fuxi = Arc::new(Fuxi::with_config(bus.clone(), ws, cfg));
        let store = TriggerStore::connect_memory().await.unwrap();
        let keeper = Arc::new(Keeper::new(
            store.clone(),
            bus.clone(),
            Arc::new(SystemClock),
        ));
        (fuxi, bus, store, keeper)
    }

    #[tokio::test]
    async fn ping_pong_roundtrip() {
        let (fuxi, bus, store, keeper) = mock_daemon_parts().await;
        let daemon = Daemon::new(fuxi, bus, store, keeper);
        let sock = temp_sock_path();

        let sock_for_server = sock.clone();
        let server = tokio::spawn(async move {
            daemon.serve(&sock_for_server).await.unwrap();
        });

        // 给 server 一点时间 bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 客户端连
        let stream = UnixStream::connect(&sock).await.expect("connect");
        let (rx, mut tx) = stream.into_split();
        let mut reader = BufReader::new(rx).lines();

        let line = serde_json::to_string(&Command::Ping).unwrap() + "\n";
        tx.write_all(line.as_bytes()).await.unwrap();
        tx.flush().await.unwrap();

        let resp = reader.next_line().await.unwrap().expect("resp");
        assert!(resp.contains("pong"), "got: {resp}");

        // 再起一条 shutdown
        let stream2 = UnixStream::connect(&sock).await.expect("connect2");
        let (rx2, mut tx2) = stream2.into_split();
        let mut reader2 = BufReader::new(rx2).lines();
        let line = serde_json::to_string(&Command::Shutdown).unwrap() + "\n";
        tx2.write_all(line.as_bytes()).await.unwrap();
        tx2.flush().await.unwrap();
        let resp = reader2.next_line().await.unwrap().expect("resp");
        assert!(resp.contains("shutdown"), "got: {resp}");

        server.await.expect("server task");
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let (fuxi, bus, store, keeper) = mock_daemon_parts().await;
        let daemon = Daemon::new(fuxi, bus, store, keeper);
        let sock = temp_sock_path();
        let shutdown = daemon.shutdown_handle();

        let sock_for_server = sock.clone();
        let server = tokio::spawn(async move {
            daemon.serve(&sock_for_server).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let stream = UnixStream::connect(&sock).await.unwrap();
        let (rx, mut tx) = stream.into_split();
        let mut reader = BufReader::new(rx).lines();
        tx.write_all(b"NOT JSON\n").await.unwrap();
        tx.flush().await.unwrap();
        let resp = reader.next_line().await.unwrap().expect("resp");
        assert!(resp.contains("error"), "got: {resp}");
        assert!(resp.contains("解析命令失败"), "got: {resp}");

        // cleanup
        shutdown.notify_waiters();
        server.await.ok();
    }
}
