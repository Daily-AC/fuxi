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
use fuxi_agent_codex::CodexLaunchConfig;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::AgentId;
use fuxi_core::task::Task;
use fuxi_events::EventBus;
use fuxi_memory::OracleStore;
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
    /// 策府——`Command::Spawn` 的 P2 召回 flag 走它查 `task-<id>` / `role-<role>`
    /// 的 `session_id` fact，转给 cc `--resume`。
    pub oracle: OracleStore,
    /// 发信号给 serve 循环的 abort——Command::Shutdown 触发。
    shutdown_signal: Arc<Notify>,
}

impl Daemon {
    pub fn new(
        fuxi: Arc<Fuxi>,
        bus: EventBus,
        store: TriggerStore,
        keeper: Arc<Keeper>,
        oracle: OracleStore,
    ) -> Self {
        Self {
            fuxi,
            bus,
            store,
            keeper,
            oracle,
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
        let oracle = self.oracle.clone();

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
                    let oracle = oracle.clone();
                    let shutdown_hook = shutdown_hook.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, fuxi, bus, store, keeper, oracle, shutdown_hook).await {
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
    oracle: OracleStore,
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
        Ok(cmd) => dispatch_command(fuxi, bus, store, keeper, oracle, cmd, shutdown_hook).await,
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
    oracle: OracleStore,
    cmd: Command,
    shutdown_hook: Arc<Notify>,
) -> Response {
    match cmd {
        Command::Ping => Response::Pong,

        Command::Spawn {
            role,
            name,
            recall_task,
            recall_role,
        } => {
            let recall = match resolve_recall_handle(&oracle, recall_task, recall_role).await {
                Ok(h) => h,
                Err(resp) => return resp,
            };
            match spawn_by_role(&fuxi, &role, name, recall).await {
                Ok(id) => Response::ok(serde_json::json!({"agent_id": id.to_string()})),
                Err(e) => Response::err(e.to_string()),
            }
        }

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
            Ok(id) => {
                // M3.7 实装：路由到 `Fuxi::shutdown_agent`——它已自带玄女豁免（命中
                // 时 warn + Ok 静默 noop）+ 不销毁 worktree（Decision 07 召回 stash）。
                // 任何新 shutdown 路径（含 `fuxi kill --id`、未来 worker pool rebalance）
                // 都必须从这条入口走，不能旁路 shelf.take。
                match fuxi
                    .shutdown_agent(id, "manual kill via fuxi kill".into())
                    .await
                {
                    Ok(()) => Response::ok(serde_json::json!({"killed": true})),
                    Err(e) => Response::err(e.to_string()),
                }
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

/// P2 召回的"可恢复 spawn 参数包"——从 oracle 里查出来的 worktree 和 session_id。
///
/// worktree 是所有 CLI 门客共用的 cwd 入口（召回的最小单位）；session_id 是 cc 专属，
/// 其他 CLI 为 None。codex 走 `worktree.is_some() && session_id.is_none()` 路径。
#[derive(Debug, Clone, Default)]
pub(crate) struct RecallHandle {
    pub resume_session_id: Option<String>,
    pub worktree: Option<PathBuf>,
}

/// 根据 role 读 Skill → 构造 AgentProfile + 对应 LaunchConfig → 丢给 Fuxi.spawn_worker
/// 或 spawn_worker_in_worktree（有召回 worktree 时）。
///
/// CLI 选择来自 `LoadedSkill.profile.cli`。新增 CLI 同步更新三处（同上版注释）。
async fn spawn_by_role(
    fuxi: &Fuxi,
    role: &str,
    name_override: Option<String>,
    recall: RecallHandle,
) -> Result<AgentId> {
    let loaded =
        skill_loader::load(role).with_context(|| format!("加载 skills/{role}/SKILL.md"))?;
    let mut profile = loaded.profile;
    if let Some(n) = name_override {
        profile.name = n;
    }

    let kind: WorkerKind = match profile.cli.as_str() {
        "claude-code" => {
            // P2 召回前提：cc 必须 persist session 才能下次 --resume 命中。
            // CcLaunchConfig 默认在 resume_session_id 和 session_id **都 None** 时
            // 加 `--no-session-persistence`——sink 记的 session_id 第二轮 resume 即死。
            // 普通 spawn（无召回）时强塞一个新 uuid 给 `session_id`：cc honor 这个 id
            // 在 system/init 事件里回报，sink 拿到的就是这个值。
            let resume_session_id = recall.resume_session_id.clone();
            let session_id = if resume_session_id.is_none() {
                Some(Uuid::new_v4().to_string())
            } else {
                None
            };
            WorkerKind::Cc(CcLaunchConfig {
                append_system_prompt: if loaded.append_system_prompt.is_empty() {
                    None
                } else {
                    Some(loaded.append_system_prompt)
                },
                allowed_tools: loaded.allowed_tools,
                resume_session_id,
                session_id,
                ..Default::default()
            })
        }
        "codex" => {
            // codex 不消化 `--append-system-prompt` / `--allowed-tools`——cc 专属。
            // codex `exec` 模式无持久 session——resume_session_id 命中 warn 后忽略，
            // 但 **worktree 复用仍有效**（L2 核心：召回 = 复用工作环境，不依赖 session）。
            if recall.resume_session_id.is_some() {
                tracing::warn!(
                    role = %role,
                    "codex 不支持 session resume；worktree 仍会复用（如有）"
                );
            }
            WorkerKind::Codex(CodexLaunchConfig::default())
        }
        other => {
            return Err(anyhow!(
                "未知 CLI 标签 '{other}'（来自 skills/{role}/SKILL.md 的 metadata.cli）；\
                 当前支持：claude-code | codex"
            ));
        }
    };

    if let Some(wt_path) = recall.worktree {
        // 召回路径：复用已有 worktree，绕过 workspace.create。
        // branch_hint 用"recall-<role>"纯标签——borrowed handle 的 destroy 短路，
        // 这个 branch 名只在事件日志/ TUI 里出现，不会走 git。
        let branch_hint = format!("recall-{role}");
        fuxi.spawn_worker_in_worktree(profile, kind, wt_path, branch_hint)
            .await
            .map_err(|e| anyhow!(e.to_string()))
    } else {
        fuxi.spawn_worker(profile, kind)
            .await
            .map_err(|e| anyhow!(e.to_string()))
    }
}

/// P2 召回：把 `--recall-task` / `--recall-role` 翻成 `RecallHandle`。
///
/// 抽出纯函数——daemon dispatch 只调它，单测可脱离 Fuxi 验。返回
/// `Result<RecallHandle, Response>`——`Err` 是可直接回客户端的错误响应；
/// 都 None 返回空 handle 走普通 spawn。
///
/// 设计点：
/// - **task_id 是入口、session_id+worktree 是值**。查 `task-<id>` / `role-<role>` 两张表。
/// - worktree 缺失：走无 worktree 召回（退化 = 普通 spawn，但保留 resume_session_id 如有）
/// - session_id 缺失（codex）：只复用 worktree，不传 resume
pub(crate) async fn resolve_recall_handle(
    oracle: &OracleStore,
    recall_task: Option<String>,
    recall_role: Option<String>,
) -> std::result::Result<RecallHandle, Response> {
    let subject = match (recall_task, recall_role) {
        (Some(_), Some(_)) => return Err(Response::err("recall_task / recall_role 互斥")),
        (Some(task_raw), None) => {
            // 容错：用户给 `task-<uuid>` 或裸 `<uuid>` 都能接。
            let core = task_raw.strip_prefix("task-").unwrap_or(&task_raw);
            format!("task-{core}")
        }
        (None, Some(role_q)) => format!("role-{role_q}"),
        (None, None) => return Ok(RecallHandle::default()),
    };

    // 同一个 subject 上查两条 predicate；至少一条命中才算有召回记录。
    let session_fact = oracle
        .query_one(&subject, "session_id")
        .await
        .map_err(|e| Response::err(e.to_string()))?;
    let worktree_fact = oracle
        .query_one(&subject, "worktree")
        .await
        .map_err(|e| Response::err(e.to_string()))?;

    if session_fact.is_none() && worktree_fact.is_none() {
        return Err(Response::err(format!("无召回记录：subject={subject}")));
    }
    Ok(RecallHandle {
        resume_session_id: session_fact.map(|f| f.object),
        worktree: worktree_fact.map(|f| PathBuf::from(f.object)),
    })
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
    use fuxi_memory::NewFact;

    /// 不起真 Fuxi——用空壳子验 wire 行为（ping/invalid cmd）。
    async fn mock_daemon_parts() -> (Arc<Fuxi>, EventBus, TriggerStore, Arc<Keeper>, OracleStore) {
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
        let oracle = OracleStore::connect_memory().await.unwrap();
        (fuxi, bus, store, keeper, oracle)
    }

    #[tokio::test]
    async fn ping_pong_roundtrip() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let daemon = Daemon::new(fuxi, bus, store, keeper, oracle);
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
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let daemon = Daemon::new(fuxi, bus, store, keeper, oracle);
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

    // ── P2 召回（resolve_recall_handle 纯函数测）──
    //
    // 测纯函数而不是端到端串 spawn_by_role——spawn_by_role 真起 cc 进程太重，
    // P2 召回的全部"决策逻辑"集中在 resolve_recall_handle 里。
    // 这种切法和 session.rs::resolve_xuannv_session 的测策略一致。

    /// cc 完整召回：`--recall-task task-abc` → 返 RecallHandle 含 session + worktree。
    /// 兼容裸 uuid（不带 `task-` 前缀）。
    #[tokio::test]
    async fn spawn_with_recall_task_resolves_full_handle_from_oracle() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        oracle
            .insert(NewFact::new("task-abc", "session_id", "sess-xyz"))
            .await
            .unwrap();
        oracle
            .insert(NewFact::new("task-abc", "worktree", "/tmp/wt-abc"))
            .await
            .unwrap();

        // 带前缀
        let h = resolve_recall_handle(&oracle, Some("task-abc".into()), None)
            .await
            .expect("Ok");
        assert_eq!(h.resume_session_id.as_deref(), Some("sess-xyz"));
        assert_eq!(
            h.worktree.as_deref().and_then(|p| p.to_str()),
            Some("/tmp/wt-abc")
        );

        // 裸 uuid 也接
        let h = resolve_recall_handle(&oracle, Some("abc".into()), None)
            .await
            .expect("Ok");
        assert_eq!(h.resume_session_id.as_deref(), Some("sess-xyz"));
    }

    /// `--recall-role dev` 取 query_one 最新（updated_at DESC）。
    #[tokio::test]
    async fn spawn_with_recall_role_picks_latest_session() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        oracle
            .insert(NewFact::new("role-dev", "session_id", "old"))
            .await
            .unwrap();
        // RFC3339 秒级精度下两条 insert 太快可能 updated_at 相等——隔 5ms 保排序。
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        oracle
            .insert(NewFact::new("role-dev", "session_id", "new"))
            .await
            .unwrap();

        let h = resolve_recall_handle(&oracle, None, Some("dev".into()))
            .await
            .expect("Ok");
        assert_eq!(h.resume_session_id.as_deref(), Some("new"));
    }

    /// 都 None → 返空 handle，daemon 走普通 spawn。
    #[tokio::test]
    async fn spawn_with_no_recall_returns_empty_handle() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let h = resolve_recall_handle(&oracle, None, None)
            .await
            .expect("Ok");
        assert!(h.resume_session_id.is_none());
        assert!(h.worktree.is_none());
    }

    /// 同时给两个 flag → 返 Err Response（互斥）。CLI 层 clap 已挡，但 wire/IPC
    /// 不走 CLI（`nc -U` 直发也算入口），所以 daemon 必须自己也守住。
    #[tokio::test]
    async fn spawn_with_both_recall_flags_errors() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let err = resolve_recall_handle(&oracle, Some("task-abc".into()), Some("dev".into()))
            .await
            .expect_err("应返 Err");
        match err {
            Response::Err { error } => assert!(error.contains("互斥"), "got: {error}"),
            other => panic!("expected Err response, got {other:?}"),
        }
    }

    /// codex 路径：oracle 只有 worktree fact 没有 session_id → 返 worktree-only handle。
    /// 这是 L2 关键场景：codex 也能进召回 wire（worktree 复用即可）。
    #[tokio::test]
    async fn spawn_with_recall_codex_returns_worktree_only_handle() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        oracle
            .insert(NewFact::new(
                "role-luban-codex",
                "worktree",
                "/tmp/wt-codex",
            ))
            .await
            .unwrap();

        let h = resolve_recall_handle(&oracle, None, Some("luban-codex".into()))
            .await
            .expect("Ok");
        assert!(h.resume_session_id.is_none(), "codex 不该有 session");
        assert_eq!(
            h.worktree.as_deref().and_then(|p| p.to_str()),
            Some("/tmp/wt-codex")
        );
    }

    /// subject 完全没记录 → 返 Err，避免静默退化让用户以为召回成功了实际是普通 spawn。
    #[tokio::test]
    async fn spawn_with_recall_no_record_errors() {
        let oracle = OracleStore::connect_memory().await.unwrap();
        let err = resolve_recall_handle(&oracle, Some("ghost".into()), None)
            .await
            .expect_err("应返 Err");
        match err {
            Response::Err { error } => assert!(error.contains("无召回记录"), "got: {error}"),
            other => panic!("expected Err, got {other:?}"),
        }
    }

    // ── M3.7 · `Command::Kill` 实装 ──
    //
    // WHY 直接走 dispatch_command 而非 socket：socket roundtrip 已被 ping_pong 验过，
    // kill 的语义重点在「Command::Kill → Fuxi::shutdown_agent → shelf 真把人拿走」
    // 这条线，避开 socket noise 让失败定位更直接。

    /// 最小 stub agent —— 不跑事件，不跑 dispatch，只占 shelf 槽位用于
    /// 验 `Command::Kill` 让 shutdown_agent 走完一遍。
    struct KillStubAgent {
        card: fuxi_core::agent::AgentCard,
    }

    impl KillStubAgent {
        fn new(role: &str) -> Arc<Self> {
            let card = fuxi_core::agent::AgentCard {
                id: AgentId::new(),
                profile: fuxi_core::agent::AgentProfile {
                    name: format!("kill-stub-{role}"),
                    role: role.to_string(),
                    cli: "stub".into(),
                    system_prompt: String::new(),
                    tags: vec![],
                    extra: Default::default(),
                },
                endpoint: "stub://kill".into(),
                status: fuxi_core::agent::AgentStatus::Idle,
            };
            Arc::new(Self { card })
        }
    }

    #[async_trait::async_trait]
    impl fuxi_core::agent::Agent for KillStubAgent {
        fn card(&self) -> &fuxi_core::agent::AgentCard {
            &self.card
        }
        async fn dispatch(
            &self,
            _task: fuxi_core::task::Task,
        ) -> fuxi_core::Result<tokio::sync::mpsc::Receiver<fuxi_core::Event>> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
        async fn send_message(
            &self,
            _task: fuxi_core::id::TaskId,
            _text: &str,
        ) -> fuxi_core::Result<()> {
            Ok(())
        }
        async fn cancel(&self, _task: fuxi_core::id::TaskId) -> fuxi_core::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> fuxi_core::Result<()> {
            Ok(())
        }
    }

    /// `Command::Kill` 把指定门客从 shelf 摘走。daemon 不再返「暂未实装」错误。
    #[tokio::test]
    async fn kill_via_command_calls_shutdown_agent() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let stub = KillStubAgent::new("dev");
        let id = fuxi.insert_agent(stub, None).await;
        assert_eq!(fuxi.worker_count().await, 1);

        let resp = dispatch_command(
            fuxi.clone(),
            bus,
            store,
            keeper,
            oracle,
            Command::Kill {
                agent_id: id.to_string(),
            },
            Arc::new(Notify::new()),
        )
        .await;

        match &resp {
            Response::Ok { data } => assert_eq!(data["killed"], serde_json::Value::Bool(true)),
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(
            fuxi.worker_count().await,
            0,
            "kill 后 shelf 应被清空——shutdown_agent 走过 take()"
        );
    }

    /// 玄女豁免——`shutdown_agent` 命中 xuannv_id 后 noop 但返 Ok。
    /// daemon kill 必须延续这条豁免（不能让 `fuxi kill --id <xuannv>` 真把她杀掉）。
    #[tokio::test]
    async fn kill_xuannv_returns_ok_but_noop() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let xuannv = KillStubAgent::new("xuannv");
        let xuannv_id = fuxi.insert_agent(xuannv, None).await;
        fuxi.set_xuannv(xuannv_id).await;
        assert_eq!(fuxi.worker_count().await, 1);

        let resp = dispatch_command(
            fuxi.clone(),
            bus,
            store,
            keeper,
            oracle,
            Command::Kill {
                agent_id: xuannv_id.to_string(),
            },
            Arc::new(Notify::new()),
        )
        .await;

        // 豁免路径走静默 Ok（shutdown_agent 早 return Ok(())），daemon 仍返 killed:true
        // 是允许的——重要的是玄女仍在 shelf 上。
        assert!(
            matches!(resp, Response::Ok { .. }),
            "豁免后 daemon 应返 Ok（即便 shutdown_agent 是 noop）；got: {resp:?}"
        );
        assert_eq!(
            fuxi.worker_count().await,
            1,
            "玄女豁免：kill 应 noop，shelf 不动"
        );
    }

    /// 无效 agent_id 字符串走 parse_agent_id 失败路径返 Err。
    #[tokio::test]
    async fn kill_with_invalid_agent_id_returns_err() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let resp = dispatch_command(
            fuxi,
            bus,
            store,
            keeper,
            oracle,
            Command::Kill {
                agent_id: "not-a-uuid".into(),
            },
            Arc::new(Notify::new()),
        )
        .await;
        match resp {
            Response::Err { error } => assert!(error.contains("无效 agent_id"), "got: {error}"),
            other => panic!("expected Err, got {other:?}"),
        }
    }
}
