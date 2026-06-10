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
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::{Task, TaskState};
use fuxi_events::EventBus;
use fuxi_memory::OracleStore;
use fuxi_orchestrator::{Fuxi, WorkerKind};
use fuxi_scheduler::store::{FireCause, NewTrigger};
use fuxi_scheduler::{Keeper, TriggerSpec, TriggerStore, new_trigger_id};
use fuxi_skills as skill_loader;
use reqwest::Client;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tokio::sync::mpsc;
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
    /// dist controller——`Command::Nodes` 的数据源；`fuxi up` 没开 `--dist-token`
    /// 时为 `None`，那时 `Command::Nodes` 直接返 err。
    pub dist: Option<Arc<crate::dist::DistController>>,
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
            dist: None,
            shutdown_signal: Arc::new(Notify::new()),
        }
    }

    /// 把 dist controller 句柄挂上来。`fuxi up` 在创建 `DistController` 之后调用
    /// （没开 `--dist-token` 就不调）。
    pub fn with_dist(mut self, dist: Arc<crate::dist::DistController>) -> Self {
        self.dist = Some(dist);
        self
    }

    /// `fuxi im start` 专用构造——home 长跑入口**总是**内嵌 controller
    /// （`im_dist::build_dist_layer` 无条件装配），所以 dist 句柄不该是 Option。
    /// 用 required arg 编译期防漏：未来 im.rs 重构者改不到这里就走不通编译。
    ///
    /// 缘起：commit 之前 im.rs 调 `Daemon::new(...)` 后**忘了链** `.with_dist(ctrl)`，
    /// 导致 `fuxi nodes` IPC 拿到 `dist=None` 报"未启用"，把玄女诊断带歪
    /// （以为 controller 没启）。
    pub fn new_for_im_start(
        fuxi: Arc<Fuxi>,
        bus: EventBus,
        store: TriggerStore,
        keeper: Arc<Keeper>,
        oracle: OracleStore,
        dist: Arc<crate::dist::DistController>,
    ) -> Self {
        Self::new(fuxi, bus, store, keeper, oracle).with_dist(dist)
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
        let dist = self.dist.clone();

        loop {
            tokio::select! {
                _ = notify.notified() => {
                    tracing::info!("daemon 收到 Shutdown 信号，停止 accept");
                    break;
                }
                res = listener.accept() => {
                    let (stream, _) = res.context("accept")?;
                    // Unix socket 没 peer addr，pid 是唯一能拿到的对端身份——给"连
                    // Command 都解不出来"的早期失败留个抓手。
                    let peer_pid = stream.peer_cred().ok().and_then(|c| c.pid());
                    let fuxi = fuxi.clone();
                    let store = store.clone();
                    let keeper = keeper.clone();
                    let bus = bus.clone();
                    let oracle = oracle.clone();
                    let dist = dist.clone();
                    let shutdown_hook = shutdown_hook.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(stream, fuxi, bus, store, keeper, oracle, dist, shutdown_hook).await {
                            tracing::warn!(error = %e, peer_pid = ?peer_pid, "connection handler errored");
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
#[allow(clippy::too_many_arguments)]
async fn handle_conn(
    stream: UnixStream,
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    store: TriggerStore,
    keeper: Arc<Keeper>,
    oracle: OracleStore,
    dist: Option<Arc<crate::dist::DistController>>,
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
        Ok(cmd) => {
            // 先抽元信息（command_kind / agent_id / task_id），dispatch_command
            // 会 move 走 cmd——拷一份短 id 给 ctx 不影响热路径。
            let ctx = command_log_ctx(&cmd);
            let resp =
                dispatch_command(fuxi, bus, store, keeper, oracle, dist, cmd, shutdown_hook).await;
            if let Response::Err { error } = &resp {
                tracing::warn!(
                    error = %error,
                    command_kind = ctx.kind,
                    agent_id = ?ctx.agent_id,
                    task_id = ?ctx.task_id,
                    "ipc 命令执行失败"
                );
            }
            resp
        }
        Err(e) => {
            tracing::warn!(error = %e, payload_len = trimmed.len(), "ipc 请求 JSON 解析失败");
            Response::err(format!("解析命令失败: {e}"))
        }
    };
    let out = serde_json::to_string(&resp)? + "\n";
    tx.write_all(out.as_bytes()).await?;
    tx.flush().await?;
    Ok(())
}

/// 从 [`Command`] 抽出供 tracing 用的元信息——拷一份短 id，让 dispatch 能 move 走 cmd。
struct CommandLogCtx {
    kind: &'static str,
    agent_id: Option<String>,
    task_id: Option<String>,
}

fn command_log_ctx(cmd: &Command) -> CommandLogCtx {
    match cmd {
        Command::Ping => CommandLogCtx {
            kind: "ping",
            agent_id: None,
            task_id: None,
        },
        Command::Spawn { .. } => CommandLogCtx {
            kind: "spawn",
            agent_id: None,
            task_id: None,
        },
        Command::Dispatch {
            agent_id, task_id, ..
        } => CommandLogCtx {
            kind: "dispatch",
            agent_id: Some(agent_id.clone()),
            task_id: task_id.clone(),
        },
        Command::Intervene { agent_id, .. } => CommandLogCtx {
            kind: "intervene",
            agent_id: Some(agent_id.clone()),
            task_id: None,
        },
        Command::Status { agent_id } => CommandLogCtx {
            kind: "status",
            agent_id: agent_id.clone(),
            task_id: None,
        },
        Command::List => CommandLogCtx {
            kind: "list",
            agent_id: None,
            task_id: None,
        },
        Command::Nodes => CommandLogCtx {
            kind: "nodes",
            agent_id: None,
            task_id: None,
        },
        Command::Kill { agent_id } => CommandLogCtx {
            kind: "kill",
            agent_id: Some(agent_id.clone()),
            task_id: None,
        },
        Command::BlockTask { task_id, .. } => CommandLogCtx {
            kind: "block_task",
            agent_id: None,
            task_id: Some(task_id.clone()),
        },
        Command::ResumeTask { task_id, .. } => CommandLogCtx {
            kind: "resume_task",
            agent_id: None,
            task_id: Some(task_id.clone()),
        },
        Command::EmitEvent { .. } => CommandLogCtx {
            kind: "emit_event",
            agent_id: None,
            task_id: None,
        },
        Command::Shutdown => CommandLogCtx {
            kind: "shutdown",
            agent_id: None,
            task_id: None,
        },
        Command::CronAdd { .. } => CommandLogCtx {
            kind: "cron_add",
            agent_id: None,
            task_id: None,
        },
        Command::CronOnce { .. } => CommandLogCtx {
            kind: "cron_once",
            agent_id: None,
            task_id: None,
        },
        Command::CronWatch { .. } => CommandLogCtx {
            kind: "cron_watch",
            agent_id: None,
            task_id: None,
        },
        Command::CronWebhook { .. } => CommandLogCtx {
            kind: "cron_webhook",
            agent_id: None,
            task_id: None,
        },
        Command::CronList => CommandLogCtx {
            kind: "cron_list",
            agent_id: None,
            task_id: None,
        },
        Command::CronFire { id } => CommandLogCtx {
            kind: "cron_fire",
            agent_id: None,
            task_id: Some(id.clone()),
        },
        Command::CronRemove { id } => CommandLogCtx {
            kind: "cron_remove",
            agent_id: None,
            task_id: Some(id.clone()),
        },
        Command::SwitchTopic { topic_id } => CommandLogCtx {
            kind: "switch_topic",
            agent_id: None,
            task_id: Some(topic_id.clone()),
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_command(
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    store: TriggerStore,
    keeper: Arc<Keeper>,
    oracle: OracleStore,
    dist: Option<Arc<crate::dist::DistController>>,
    cmd: Command,
    shutdown_hook: Arc<Notify>,
) -> Response {
    match cmd {
        Command::Ping => Response::Pong,

        Command::Spawn {
            role,
            name,
            node,
            cli,
            recall_task,
            recall_role,
            project,
            ephemeral_task,
        } => {
            let recall = match resolve_recall_handle(&oracle, recall_task, recall_role).await {
                Ok(h) => h,
                Err(resp) => return resp,
            };
            match spawn_by_role(
                &fuxi,
                &role,
                name,
                node,
                cli,
                recall,
                project,
                ephemeral_task,
            )
            .await
            {
                Ok(id) => Response::ok(serde_json::json!({"agent_id": id.to_string()})),
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::Dispatch {
            agent_id,
            task_id,
            title,
            body,
            pinned_node,
            required_tags,
            topic_id,
        } => match parse_agent_id(&agent_id) {
            Err(e) => Response::err(e),
            Ok(id) => {
                // PR-C 反演 issue 5b7c99a7：pinned_node 落到 controller 不认识
                // 的 node_id（玄女把 "mac-local" 当 alias 用，但实际 node 叫
                // "zyldemacbook-pro-local"）会让 task 入 dist queue 永远 queued。
                // 这里早早拒绝，错误信息明示让玄女去 `fuxi nodes` 查真实 id。
                //
                // `dist=None` 时不挡——`fuxi up` 无 --dist-token 是另一条 bug
                // (issue 42184579 已 awaiting_test)；本检查只覆盖 controller 启用
                // 但 node_id 拼错的场景。
                if let Some(pinned) = pinned_node.as_deref()
                    && let Some(ctrl) = &dist
                {
                    let snapshots = ctrl.nodes_snapshot().await;
                    let alive_match = snapshots
                        .iter()
                        .any(|n| n.node_id == pinned && n.status == "alive");
                    if !alive_match {
                        return Response::err(format!(
                            "no alive node matches pinned_node={pinned}——用 `fuxi nodes` 查可用 node_id"
                        ));
                    }
                }
                let desc = body.as_deref().unwrap_or("");
                if let Some(parent_raw) = task_id {
                    // β · #70 v1 限制：dispatch_in_task 走 dispatch_to_any 路径，
                    // 暂不传 pinned_node/required_tags（fan-out 到多 worker 时
                    // routing 语义另说）。同 task fan-out 撞 dist 需求时再扩。
                    if pinned_node.is_some() || !required_tags.is_empty() {
                        tracing::warn!(
                            "dispatch --task <parent> 同时带 pinned_node/required_tags v1 暂忽略 routing hint"
                        );
                    }
                    match parse_task_id(&parent_raw) {
                        Err(e) => Response::err(e),
                        Ok(parent) => match fuxi.dispatch_in_task(id, parent, &title, desc).await {
                            Ok(()) => {
                                Response::ok(serde_json::json!({"task_id": parent.to_string()}))
                            }
                            Err(e) => Response::err(e.to_string()),
                        },
                    }
                } else {
                    // β · #70 把 CLI 的 routing hint 注入 Task → Fuxi::dispatch 决策
                    // 树命中 dist enqueue（pinned_node.is_some() || !tags.is_empty()）。
                    let mut task = Task::new(&title, desc);
                    if let Some(node) = pinned_node {
                        task = task.with_pinned_node(node);
                    }
                    if !required_tags.is_empty() {
                        task = task.with_required_tags(required_tags);
                    }
                    // 块5：玄女分身经 FUXI_TOPIC/--topic 带来的归属 topic → task.topic_id，
                    // 让门客事件 meta.topic_id 归位该 topic（适配器已消费 task.topic_id，
                    // bridge 据此路由到归属分身）。解析失败只 warn 不挂（不阻断派活）。
                    if let Some(raw) = topic_id.as_deref().filter(|s| !s.trim().is_empty()) {
                        match uuid::Uuid::parse_str(raw.trim()) {
                            Ok(u) => task = task.with_topic_id(fuxi_core::TopicId::from(u)),
                            Err(e) => {
                                tracing::warn!(topic = raw, error = %e, "dispatch --topic 非法 UUID，本次不挂 topic")
                            }
                        }
                    }
                    let task_id = task.id;
                    match fuxi.dispatch(id, task).await {
                        Ok(()) => Response::ok(serde_json::json!({"task_id": task_id.to_string()})),
                        Err(e) => Response::err(e.to_string()),
                    }
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
                // daemon CLI 入口（fuxi-cli 子命令）—— v3 #N7' mentions 仅 PWA 用，
                // CLI 不带 @；attachments 也仅 PWA 用，传空 Vec 维持现有 wire 语义。
                match fuxi
                    .intervene(
                        id,
                        interrupt_first,
                        &text,
                        Vec::new(),
                        None,
                        Vec::new(),
                        None,
                    )
                    .await
                {
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

        Command::Nodes => match dist {
            // 措辞历史教训：早期版本写"`fuxi up` 缺 --dist-token / $FUXI_DIST_TOKEN"
            // 把玄女在 `fuxi im start` 部署上的诊断带歪——她以为 controller 没启，
            // 真因是 daemon wiring 漏 `.with_dist(...)`。现说"未注入 controller 句柄"
            // 中性描述实际状态，括号给两条可能入口避免再误导。
            None => Response::err(
                "dist controller 未启用——daemon 未注入 controller 句柄（`fuxi up` 需 --dist-token / $FUXI_DIST_TOKEN；`fuxi im start` 走 `Daemon::new_for_im_start`）",
            ),
            Some(ctrl) => {
                let snapshots = ctrl.nodes_snapshot().await;
                Response::ok(serde_json::json!({ "nodes": snapshots }))
            }
        },

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
            use fuxi_core::{Event, EventKind, EventMeta};
            let ev_kind = kind.into_event_kind();
            // Jarvis 语音线：必须把 `meta.agent = xuannv_id`，否则 conv WS（按
            // `meta.agent==xuannv` 过滤透传）拿不到事件。其他 EmitEvent 变体
            // （招贤一族）是平台事件 meta.agent 留 None 才正确。
            let mut meta = EventMeta::now();
            if matches!(ev_kind, EventKind::XuannvVoiceLine { .. }) {
                meta.agent = fuxi.xuannv_id().await;
            }
            let ev = Event {
                meta,
                kind: ev_kind,
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

        Command::SwitchTopic { topic_id } => match handle_switch_topic(&fuxi, &topic_id).await {
            Ok(()) => Response::ok(serde_json::json!({"switched": true, "topic_id": topic_id})),
            Err(e) => Response::err(e.to_string()),
        },
    }
}

/// daemon 端 `Command::SwitchTopic` 入口：临时开 im.db pool 调
/// `topic_switch::switch_topic_to`。pool 短跑用完就丢，跟 fuxi-im start 那份长跑
/// pool 不冲突（SQLite WAL 支持多 reader/writer，且同一 DB 路径 sqlx 内部 connection
/// pool 不共享是 OK 的——schema 升级走 migration 幂等）。
///
/// Phase 2：切换不再 spawn，oracle/conv_store/role 都移交 `ensure_xuannv_for_topic`
/// 内部的 `TopicXuannvSpawner`，本入口只需 topic_store 验在 + topic_id。
async fn handle_switch_topic(fuxi: &Arc<Fuxi>, topic_id_str: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use fuxi_im::topic_store::TopicStore;

    let uuid = Uuid::parse_str(topic_id_str)
        .with_context(|| format!("topic_id 非 UUID 形态：{topic_id_str}"))?;
    let topic_id = fuxi_core::TopicId::from(uuid);

    let db_path = fuxi_im::db::default_db_path().context("$HOME 未设——无法定位 ~/.fuxi/im.db")?;
    let pool = fuxi_im::db::init_at(&db_path)
        .await
        .with_context(|| format!("打开 im.db {} 失败", db_path.display()))?;
    let topics = TopicStore::new(pool);

    crate::topic_switch::switch_topic_to(fuxi, &topics, topic_id).await
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

#[derive(Debug, Clone)]
struct DistGatewayConfig {
    controller: String,
    /// enqueue 时写入 job 作 "requester hint"（日志/审计）。3b 之后不影响派工。
    node_id: String,
    /// 旧 token——生产 authn 已切 HMAC（lazily-loaded `HmacSecret` 在 dispatch
    /// 入口现取 from_env），仅 progress GET 的 query 兼容字段使用。新代码
    /// 不要读它做鉴权。
    token: String,
    poll_ms: u64,
    /// role 声明需要的 worker 能力——enqueue 透传，派工时 `pull` 端匹配。
    /// 空集 = 无能力要求，任一 idle worker 可取。
    required_tags: Vec<String>,
    /// 硬 pin：只有 `worker.node_id == x` 能取此 job。CLI `--node` 或
    /// role metadata.dist_node 明确声明时填（env 默认不 pin）。
    pinned_node: Option<String>,
    /// 指定 worker 上用哪个 CLI 跑——源自 role profile.cli（`"claude-code"` /
    /// `"codex"`）。worker 端根据它选 CliAdapter；老版 worker 不认识就
    /// fallback 到 codex（Phase 4a select_adapter 语义）。
    cli: String,
    /// cc 专属参数——role 的 allowed-tools frontmatter，以 `--allowed-tools`
    /// 传给 worker 端的 claude-code adapter。codex 路径忽略。
    allowed_tools: Vec<String>,
    /// home 端 dispatch 时的目标 role；transparently 透传到 worker 让 worker
    /// 端能 publish AgentSpawning 拼对 role 标签。**也是** Decision 21 phase 3
    /// L3 sandbox 的 sandbox 索引——worker 端 `<root>/<project>/sandboxes/<role>/`。
    role: Option<String>,
    /// Decision 21 phase 3：跨节点项目 slug。Some 时 worker 端把 cc/codex 起在
    /// 自己 ProjectRegistry 里同名 project 的 sandbox/L2 worktree 里。
    project: Option<String>,
    /// Decision 21 phase 3：L2 一次性 task 显示形（`task-<uuid>`）。
    /// 配 `project` 一起才有意义；单独给 worker 忽略。
    ephemeral_task: Option<String>,
}

/// 远端网关门客：把 dispatch 转成 `/dist/enqueue`，再轮询 `/dist/progress` 拿增量。
///
/// 这让玄女保持现有 spawn/dispatch 心智，底层可把 codex 执行下沉到公网网关节点。
struct DistGatewayAgent {
    card: AgentCard,
    cfg: DistGatewayConfig,
    /// 活跃的 `task_id → job_id` 映射——cancel 时据此回查 controller 的 job id。
    ///
    /// 每次 dispatch 在拿到 enqueue 响应后插入，dispatch 完成（无论 ok / err /
    /// cancel）时移除。并发 dispatch 合法（Task 层可能把一个 agent 当 one-shot
    /// spawn，理论上不会并发，但 shelf 回收慢时容许这个组合）。
    active: Arc<tokio::sync::Mutex<HashMap<TaskId, String>>>,
}

impl DistGatewayAgent {
    fn new(id: AgentId, profile: AgentProfile, cfg: DistGatewayConfig) -> Self {
        Self {
            card: AgentCard {
                id,
                profile,
                endpoint: format!("dist://{}@{}", cfg.node_id, cfg.controller),
                status: AgentStatus::Idle,
            },
            cfg,
            active: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl Agent for DistGatewayAgent {
    fn card(&self) -> &AgentCard {
        &self.card
    }

    async fn dispatch(&self, task: Task) -> fuxi_core::Result<mpsc::Receiver<Event>> {
        let (tx, rx) = mpsc::channel::<Event>(32);
        let cfg = self.cfg.clone();
        let aid = self.card.id;
        let active = self.active.clone();
        // role 心智来自 loader 写入的 profile.system_prompt（同本地 CodexAgent）
        // ——worker 侧会 prepend 到 prompt 头部。空串则不填，省 bytes。
        let system_prompt = {
            let sp = self.card.profile.system_prompt.trim();
            if sp.is_empty() {
                None
            } else {
                Some(sp.to_string())
            }
        };
        tokio::spawn(async move {
            let client = Client::new();
            let controller = cfg.controller.trim_end_matches('/').to_string();
            // secret 在 dispatch 入口取——缺 env 直接走 terminal error，避免
            // build_dist_gateway_config 单测因 env 强约束而炸。
            let secret = match load_dist_secret() {
                Ok(s) => s,
                Err(e) => {
                    let _ = emit_terminal_error(&tx, aid, task.id, format!("{e}")).await;
                    return;
                }
            };
            let body = if task.description.trim().is_empty() {
                task.title.clone()
            } else {
                task.description.clone()
            };
            let enqueue_url = format!("{controller}/dist/enqueue");
            let enqueue_req = crate::dist::DistEnqueueReq {
                node_id: cfg.node_id.clone(),
                title: task.title.clone(),
                body,
                system_prompt,
                required_tags: cfg.required_tags.clone(),
                pinned_node: cfg.pinned_node.clone(),
                cli: cfg.cli.clone(),
                allowed_tools: cfg.allowed_tools.clone(),
                // daemon gateway 路径透传 task 真相（#76）；role 暂走 None
                // （cfg 没 role_hint 字段——daemon 路径用户场景较少，#77 主路径
                // 在 fuxi-im DistControllerEnqueuer 上）
                task_id: Some(task.id.to_string()),
                role: cfg.role.clone(),
                // Decision 21 phase 3：跨节点 sandbox 透传——worker 端用 project
                // 解 ProjectRegistry 拿 sandbox cwd 把 cc/codex 起在那里。
                project: cfg.project.clone(),
                ephemeral_task: cfg.ephemeral_task.clone(),
                // FU-2 跨节点收尾：透传发起 task 的 topic，让远端 worker 事件归位。
                topic_id: task.topic_id.map(|t| t.to_string()),
            };
            let enq =
                crate::dist_auth_client::signed_post(&client, &secret, &enqueue_url, &enqueue_req)
                    .await
                    .and_then(|resp| resp.error_for_status().map_err(anyhow::Error::from));

            let job_id = match enq {
                Ok(resp) => match resp.json::<crate::dist::DistEnqueueResp>().await {
                    Ok(v) => v.job_id,
                    Err(e) => {
                        let _ = emit_terminal_error(
                            &tx,
                            aid,
                            task.id,
                            format!("dist enqueue decode failed: {e}"),
                        )
                        .await;
                        return;
                    }
                },
                Err(e) => {
                    let _ =
                        emit_terminal_error(&tx, aid, task.id, format!("dist enqueue failed: {e}"))
                            .await;
                    return;
                }
            };

            // 记 active 映射供 cancel 回查 job_id。loop 结束时统一移除
            // （见 'body 标签的 break）。
            active.lock().await.insert(task.id, job_id.clone());

            // 流式轮询 progress：每拿到一批增量 chunk 就按 kind emit 事件；
            // done=true 时 emit 终态。不再走老的"一次性 AgentResponded(final_output)"
            // ——长任务用户不用等黑盒，每几百毫秒能看到增量。
            let mut cursor: u64 = 0;
            'body: loop {
                tokio::time::sleep(std::time::Duration::from_millis(cfg.poll_ms)).await;
                let cursor_str = cursor.to_string();
                let progress_url = format!("{controller}/dist/progress");
                // query 不在签名保护内（α middleware 仅签 path），但 token 已弃用，
                // job_id / after 仅作 routing hint——controller 用 HMAC 验身份，
                // 不再凭 query token 鉴权。保留 token query 仅为兼容尚未升级的
                // 旧 controller，新版会无视。
                let poll = crate::dist_auth_client::signed_get(
                    &client,
                    &secret,
                    &progress_url,
                    &[
                        ("token", cfg.token.as_str()),
                        ("job_id", job_id.as_str()),
                        ("after", cursor_str.as_str()),
                    ],
                )
                .await
                .and_then(|resp| resp.error_for_status().map_err(anyhow::Error::from));
                let resp = match poll {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let status = match resp.json::<crate::dist::DistProgressResp>().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                for chunk in &status.chunks {
                    if chunk.seq > cursor {
                        cursor = chunk.seq;
                    }
                    let kind = progress_chunk_to_event_kind(chunk, aid, task.id);
                    let _ = emit_event(&tx, aid, task.id, kind).await;
                }
                if !status.done {
                    continue;
                }
                let _ = emit_event(
                    &tx,
                    aid,
                    task.id,
                    EventKind::TaskStateChanged {
                        from: TaskState::InProgress,
                        to: TaskState::Delivering,
                    },
                )
                .await;
                let terminal = if status.final_ok.unwrap_or(false) {
                    EventKind::TaskStateChanged {
                        from: TaskState::Delivering,
                        to: TaskState::Done,
                    }
                } else {
                    // 失败 path：把 final_output 作为错误摘要补发（chunks 里可能
                    // 已经 Error 过，但 final_output 常含退出码信息，一并 emit
                    // 给用户完整上下文）。
                    if let Some(txt) = status.final_output.as_deref()
                        && !txt.trim().is_empty()
                    {
                        let _ = emit_event(
                            &tx,
                            aid,
                            task.id,
                            EventKind::AgentResponded {
                                text: format!("[final] {txt}"),
                                artifact_ref: None,
                            },
                        )
                        .await;
                    }
                    EventKind::TaskStateChanged {
                        from: TaskState::InProgress,
                        to: TaskState::Cancelled,
                    }
                };
                let _ = emit_event(&tx, aid, task.id, terminal).await;
                break 'body;
            }
            active.lock().await.remove(&task.id);
        });
        Ok(rx)
    }

    async fn send_message(&self, _task_id: TaskId, _text: &str) -> fuxi_core::Result<()> {
        Err(fuxi_core::CoreError::Other(
            "dist gateway worker does not support send_message yet".into(),
        ))
    }

    async fn cancel(&self, task_id: TaskId) -> fuxi_core::Result<()> {
        // 查活跃 task→job 映射；无匹配视作 no-op（本地/分布式混合 shelf 下
        // 上层可能向任意 agent 发 cancel，不该因为 map miss 就报错）。
        let job_id = match self.active.lock().await.get(&task_id).cloned() {
            Some(id) => id,
            None => {
                tracing::debug!(agent = %self.card.id, task = %task_id, "dist gateway cancel ignored: no active job");
                return Ok(());
            }
        };
        let url = format!("{}/dist/cancel", self.cfg.controller.trim_end_matches('/'));
        let cancel_req = crate::dist::DistCancelReq {
            job_id: job_id.clone(),
        };
        let secret = load_dist_secret().map_err(|e| fuxi_core::CoreError::Other(format!("{e}")))?;
        let res =
            crate::dist_auth_client::signed_post(&Client::new(), &secret, &url, &cancel_req).await;
        match res {
            Ok(r) if r.status().is_success() => {
                tracing::info!(agent = %self.card.id, task = %task_id, job = %job_id, "dist gateway cancel posted");
                Ok(())
            }
            Ok(r) => Err(fuxi_core::CoreError::Other(format!(
                "dist gateway cancel non-2xx: {}",
                r.status()
            ))),
            Err(e) => Err(fuxi_core::CoreError::Other(format!(
                "dist gateway cancel failed: {e}"
            ))),
        }
    }

    async fn shutdown(&self) -> fuxi_core::Result<()> {
        Ok(())
    }
}

/// progress chunk → EventKind 映射。普通 kind 都走 `AgentResponded` 通道，
/// 仅靠 `[tool]` / `[thinking]` / `[error]` 文本前缀区分；TUI 将来可以据此上色。
///
/// 为什么不映射到 `ThinkingStarted` / `ToolCallStarted`：那两条都是独立 lifecycle
/// 事件，codex wire 里我们只拿得到 **completed** 阶段（reasoning/item），缺
/// started 就让 TUI 状态机为难。全走 AgentResponded + 前缀是最少破坏的选择，
/// Phase 3+ 真要分层渲染时再细化。
///
/// **例外（issue 18dfccea）**：`AssistantText` chunk 若是 `_fuxi:request_review`
/// sentinel，必须翻成 `AgentRequestReview`——dist gateway 路径不经本地 cc
/// parser，sentinel 识别逻辑在这里复用 `fuxi_agent_cc::parser`。漏掉这一步则
/// 跨节点门客的交付 nudge 被当普通 `AgentResponded` 透传，bridge 白名单不含
/// `AgentResponded`，玄女永远收不到 `[REVIEW_REQUEST]`。
fn progress_chunk_to_event_kind(
    chunk: &crate::dist::ProgressChunk,
    agent_id: AgentId,
    task_id: TaskId,
) -> EventKind {
    if matches!(chunk.kind, crate::dist::ProgressKind::AssistantText)
        && let Some(sentinel) =
            fuxi_agent_cc::parser::try_parse_request_review_sentinel(&chunk.text)
    {
        return EventKind::AgentRequestReview {
            agent: agent_id,
            task: task_id,
            deliverable_kind: sentinel.kind,
            summary: sentinel.summary,
            artifact_ref: sentinel.artifact_ref,
        };
    }
    let text = match chunk.kind {
        crate::dist::ProgressKind::AssistantText => chunk.text.clone(),
        crate::dist::ProgressKind::Thinking => format!("[thinking] {}", chunk.text),
        crate::dist::ProgressKind::ToolCall => format!("[tool] {}", chunk.text),
        crate::dist::ProgressKind::Error => format!("[error] {}", chunk.text),
    };
    EventKind::AgentResponded {
        text,
        artifact_ref: None,
    }
}

async fn emit_event(
    tx: &mpsc::Sender<Event>,
    agent_id: AgentId,
    task_id: fuxi_core::TaskId,
    kind: EventKind,
) -> std::result::Result<(), ()> {
    let mut meta = EventMeta::now();
    meta.agent = Some(agent_id);
    meta.task = Some(task_id);
    tx.send(Event { meta, kind }).await.map_err(|_| ())
}

async fn emit_terminal_error(
    tx: &mpsc::Sender<Event>,
    agent_id: AgentId,
    task_id: fuxi_core::TaskId,
    msg: String,
) -> std::result::Result<(), ()> {
    emit_event(
        tx,
        agent_id,
        task_id,
        EventKind::AgentResponded {
            text: format!("远端网关执行失败：{msg}"),
            artifact_ref: None,
        },
    )
    .await?;
    emit_event(
        tx,
        agent_id,
        task_id,
        EventKind::TaskStateChanged {
            from: TaskState::InProgress,
            to: TaskState::Cancelled,
        },
    )
    .await
}

/// cangjie / extractor 是平台**内部角色**——只由经验抽取管线（InsightExtractorTask /
/// FactExtractor）经 `dispatch_to_any_in_task` 在进程内唤起，喂的是 trajectory 抽取
/// prompt。它们的 ROLE.md 契约是"非 trajectory 输入直接返 `[]`"，所以被用户/玄女经
/// `fuxi spawn` 错派通用调研任务时会**静默空跑**，玄女误以为搞定（issue fb09ee62）。
/// 在 spawn 入口 fail loud，引导改派交付型门客。
fn is_internal_only_role(role: &str) -> bool {
    matches!(role.trim(), "cangjie" | "extractor")
}

/// 根据 role 读 Skill → 构造 AgentProfile + 对应 LaunchConfig → 丢给 Fuxi.spawn_worker
/// 或 spawn_worker_in_worktree（有召回 worktree 时）。
///
/// CLI 选择来自 `LoadedSkill.profile.cli`。新增 CLI 同步更新三处（同上版注释）。
#[allow(clippy::too_many_arguments)]
async fn spawn_by_role(
    fuxi: &Fuxi,
    role: &str,
    name_override: Option<String>,
    node_override: Option<String>,
    cli_override: Option<String>,
    recall: RecallHandle,
    project_override: Option<String>,
    ephemeral_task_override: Option<String>,
) -> Result<AgentId> {
    // issue fb09ee62：内部角色不接受直接派活——否则错派通用任务时静默返 [] 让玄女
    // 误判完成。内部抽取管线不走这条路径（用 dispatch_to_any_in_task），guard 安全。
    if is_internal_only_role(role) {
        anyhow::bail!(
            "'{role}' 是平台内部角色（经验抽取管线专用），不接受直接 spawn 派活；通用调研/写代码请派 luban / pusong 等交付型门客"
        );
    }
    let loaded = skill_loader::load(role).with_context(|| format!("加载 roles/{role}/ROLE.md"))?;
    let mut profile = loaded.profile;
    if let Some(n) = name_override {
        profile.name = n;
    }
    let metadata_json =
        serde_json::to_value(&loaded.frontmatter.metadata).unwrap_or(serde_json::Value::Null);
    let requested_node = normalize_node(node_override);
    let requested_cli = normalize_cli(cli_override)?;
    let cli = requested_cli.unwrap_or_else(|| profile.cli.clone());
    profile.cli = cli.clone();

    // Phase 4b: cc 也允许走分布式——cli-specific 的 "远端只支持 codex" 禁令取消。
    // worker 端按 job.cli 选 CliAdapter（Phase 4a），cc 走 Phase 4c 的 stdout
    // stream-json MVP（无 WS 反连，避 Clash TUN 的坑；follow-up/resume 不支持）。
    let force_local = requested_node.as_deref().is_some_and(|n| n == "local");
    if !force_local
        && let Some(mut cfg) = build_dist_gateway_config(&metadata_json, requested_node.as_deref())?
    {
        cfg.cli = cli.clone();
        cfg.allowed_tools = loaded.allowed_tools.clone().unwrap_or_default();
        cfg.role = Some(role.to_string());
        // Decision 21 phase 3：跨节点 sandbox——把 CLI / 玄女 dispatch 时给的
        // project / ephemeral_task 透传到 worker，让 worker 端解 ProjectRegistry
        // 把 cc/codex 起在对应 sandbox 里。worker 节点必须 pre-`fuxi project add`
        // 同名 slug，否则 worker pull 后 fail job（明确 error 比 silent fallback 好）。
        cfg.project = project_override.clone();
        cfg.ephemeral_task = ephemeral_task_override.clone();
        apply_project_tag_to_required_tags(&mut cfg.required_tags, project_override.as_deref());
        if recall.resume_session_id.is_some() || recall.worktree.is_some() {
            tracing::warn!(
                role = %role,
                cli = %cli,
                "dist gateway 暂不支持 recall；忽略 recall_task/recall_role"
            );
        }
        if cli == "claude-code" {
            tracing::warn!(
                role = %role,
                "dist gateway claude-code 当前只支持 one-shot；follow-up/resume 自动退化为新 dispatch"
            );
        }
        let id = AgentId::new();
        let agent = Arc::new(DistGatewayAgent::new(id, profile, cfg)) as Arc<dyn Agent>;
        return Ok(fuxi.insert_agent(agent, None).await);
    }

    let kind: WorkerKind = match cli.as_str() {
        "claude-code" => {
            // 普通 spawn（无召回）时**不**强塞 `session_id`——cc 2.1.114+ 在
            // `--sdk-url` SDK 模式下把 `--session-id <uuid>` 当 strict resume 解释，
            // session 不存在就直接 stderr `No conversation found with session ID: <uuid>`
            // 退出，fuxi-im 看不到死亡只看到 WS Connection reset；玄女永久 zombie。
            // （cc stdio 模式下 `--session-id <new-uuid>` 仍允许新建，单测和 fuxi 路径
            // 走 SDK 模式踩坑。）
            //
            // 让 cc 自己生成 session_id，`ws_bridge` 已经从 `system/init` 事件 peek
            // session_id 写进 `cli_session_id` slot（见 `ws_bridge.rs::287`）；P2 召回
            // 路径继续从 sink 的 `cli_session_id` fact 拿值传 `resume_session_id`。
            let resume_session_id = recall.resume_session_id.clone();
            WorkerKind::Cc(CcLaunchConfig {
                append_system_prompt: if loaded.append_system_prompt.is_empty() {
                    None
                } else {
                    Some(loaded.append_system_prompt)
                },
                allowed_tools: loaded.allowed_tools,
                disallowed_tools: loaded.disallowed_tools,
                resume_session_id,
                session_id: None,
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
                "未知 CLI 标签 '{other}'（来自 roles/{role}/ROLE.md 的 metadata.cli）；\
                 当前支持：claude-code | codex"
            ));
        }
    };

    if let Some(wt_path) = recall.worktree {
        // 召回路径：复用已有 worktree，绕过 workspace.create。
        // branch_hint 用"recall-<role>"纯标签——borrowed handle 的 destroy 短路，
        // 这个 branch 名只在事件日志/ TUI 里出现，不会走 git。
        let branch_hint = format!("recall-{role}");
        return fuxi
            .spawn_worker_in_worktree(profile, kind, wt_path, branch_hint)
            .await
            .map_err(|e| anyhow!(e.to_string()));
    }

    // Decision 21 phase 3：CLI `--project erp --ephemeral --task task-...` →
    // 走 L2 ephemeral 路径（per-task 临时 worktree，task 死即归档/回收）。
    // 优先级高于 L3——明示 ephemeral 时不退回默认 L3。
    if let (Some(project_slug), Some(task_raw)) =
        (project_override.as_ref(), ephemeral_task_override.as_ref())
    {
        let project_id = fuxi_core::ProjectId::new(project_slug.clone())
            .with_context(|| format!("无效 project slug: {project_slug}"))?;
        let trimmed = task_raw.strip_prefix("task-").unwrap_or(task_raw);
        let task_uuid =
            Uuid::parse_str(trimmed).with_context(|| format!("无效 task uuid '{task_raw}'"))?;
        return fuxi
            .spawn_worker_in_ephemeral_workspace(
                profile,
                kind,
                project_id,
                fuxi_core::TaskId::from(task_uuid),
            )
            .await
            .map_err(|e| anyhow!(e.to_string()));
    }

    // Decision 21 phase 1：CLI `--project erp` → 走 L3 持久 sandbox 路径
    // （per-门客 per-project 长期 worktree，跨 task 复用 build cache + WIP）。
    // role 同 sandbox 索引——一个项目的 luban 只有一个 sandbox，重复 spawn 即复用。
    if let Some(project_slug) = project_override {
        let project_id = fuxi_core::ProjectId::new(project_slug.clone())
            .with_context(|| format!("无效 project slug: {project_slug}"))?;
        return fuxi
            .spawn_worker_in_project_sandbox(profile, kind, project_id, role.to_string())
            .await
            .map_err(|e| anyhow!(e.to_string()));
    }

    fuxi.spawn_worker(profile, kind)
        .await
        .map_err(|e| anyhow!(e.to_string()))
}

/// Decision 21 phase 3 tag-aware spawn：dist 路径下若指定了 project，
/// 自动加 `project:<slug>` 到 required_tags——controller 用 tag 子集匹配
/// 把 job 路由到 advertise 该 tag 的 worker。这让用户在 worker 节点上跑
/// `fuxi dist worker --node home --tag project:erp` 即可声明"我托管 erp"，
/// 后续 `fuxi spawn --node home --project erp` 才能命中。pinned_node 优先
/// 时此 tag 仍写入但被忽略——pinned 已经定死目标，多写一个 tag 无害。
///
/// 拆独立 helper 让单测能直接验证"slug 为空 / 已存在 tag / 多次幂等"等边界。
fn apply_project_tag_to_required_tags(tags: &mut Vec<String>, project_slug: Option<&str>) {
    let Some(slug) = project_slug.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let project_tag = format!("project:{slug}");
    if tags.iter().any(|t| t == &project_tag) {
        return;
    }
    tags.push(project_tag);
}

fn build_dist_gateway_config(
    metadata: &serde_json::Value,
    node_override: Option<&str>,
) -> Result<Option<DistGatewayConfig>> {
    let metadata_controller = metadata_string(metadata, "dist_controller");
    let env_controller = std::env::var("FUXI_DIST_CONTROLLER").ok();
    let controller = metadata_controller.or(env_controller);

    // pin 语义：CLI `--node` 或 role metadata `dist_node` 是**显式声明**——pin。
    // 仅 env `FUXI_DIST_NODE` 则只作 hint，不 pin（env 往往用作默认值，不该
    // 变成硬限制把所有 role 锁死在同一 worker）。
    let cli_node = node_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let metadata_node = metadata_string(metadata, "dist_node");
    let env_node = std::env::var("FUXI_DIST_NODE").ok();
    let pinned_node = cli_node.clone().or_else(|| metadata_node.clone());
    let node_id_hint = pinned_node.clone().or(env_node);

    let wants_dist = node_override.is_some() || controller.is_some() || metadata_node.is_some();
    if !wants_dist {
        return Ok(None);
    }

    let controller = controller.ok_or_else(|| {
        anyhow!("缺 dist controller：请配置 metadata.dist_controller 或 $FUXI_DIST_CONTROLLER")
    })?;
    let node_id = node_id_hint.ok_or_else(|| {
        anyhow!("缺 dist node：请配置 metadata.dist_node 或 --node / $FUXI_DIST_NODE")
    })?;

    let token = metadata_string(metadata, "dist_token")
        .or_else(|| node_scoped_dist_token_env(&node_id).and_then(read_env))
        .or_else(|| std::env::var(crate::dist::DIST_TOKEN_ENV).ok())
        .ok_or_else(|| {
            let scoped = node_scoped_dist_token_env(&node_id)
                .unwrap_or_else(|| "FUXI_DIST_<NODE>_TOKEN".to_string());
            anyhow!(
                "缺 dist token：请配置 metadata.dist_token 或 ${} 或 ${scoped}",
                crate::dist::DIST_TOKEN_ENV
            )
        })?;
    // HMAC 取代 token 作 dist 鉴权。**不在此处加载 secret**——build 阶段是纯
    // metadata 解析，单测不必 set env 才能走通；secret 在 dispatch / cancel 入口
    // 经 `cfg_secret_or_emit` 取 from_env，缺则 emit 终态错误而不 panic。
    let poll_ms = metadata_u64(metadata, "dist_poll_ms").unwrap_or(1000);
    let required_tags = metadata_string_vec(metadata, "required_tags");
    Ok(Some(DistGatewayConfig {
        controller,
        node_id,
        token,
        poll_ms,
        required_tags,
        pinned_node,
        // cli / allowed_tools / role / project / ephemeral_task 调用方
        // （spawn_by_role）会从 loaded + CLI flag 补上——build_dist_gateway_config
        // 职责纯粹，只看 metadata。
        cli: String::new(),
        allowed_tools: Vec::new(),
        role: None,
        project: None,
        ephemeral_task: None,
    }))
}

/// 取 HMAC secret——缺 env 时把错误信息嵌入 emit 路径而不是 panic。
/// dispatch / cancel 都靠这个 helper 拿 secret，单点失败语义统一。
fn load_dist_secret() -> anyhow::Result<Arc<crate::dist_auth::HmacSecret>> {
    crate::dist_auth::HmacSecret::from_env()
        .map(Arc::new)
        .map_err(|e| anyhow!("dist gateway HMAC secret: {e}"))
}

/// 读 metadata 里的字符串数组字段，空/缺失都返回空 Vec。
///
/// Phase 0 删过一个同名函数（那个支持字符串 split 分支，只给 ssh 路径用）；
/// 这个新版只接数组——role metadata 的 `required_tags: ["codex"]` 用 TOML 数组
/// 写最自然，不再支持空格分隔字符串（那属于 CLI 世界，不该泄漏到 role 文件）。
fn metadata_string_vec(metadata: &serde_json::Value, key: &str) -> Vec<String> {
    metadata
        .as_object()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_node(node: Option<String>) -> Option<String> {
    node.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn normalize_cli(cli: Option<String>) -> Result<Option<String>> {
    let Some(raw) = cli else {
        return Ok(None);
    };
    let v = raw.trim().to_string();
    if v.is_empty() {
        return Ok(None);
    }
    match v.as_str() {
        "claude-code" | "codex" => Ok(Some(v)),
        other => Err(anyhow!(
            "未知 CLI 覆写 '{other}'；当前支持 claude-code | codex"
        )),
    }
}

fn node_scoped_dist_token_env(node_id: &str) -> Option<String> {
    let key = node_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if key.is_empty() {
        None
    } else {
        Some(format!("FUXI_DIST_{key}_TOKEN"))
    }
}

fn read_env(key: String) -> Option<String> {
    std::env::var(key).ok()
}

fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .as_object()
        .and_then(|m| m.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_u64(metadata: &serde_json::Value, key: &str) -> Option<u64> {
    metadata
        .as_object()
        .and_then(|m| m.get(key))
        .and_then(serde_json::Value::as_u64)
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

    // issue fb09ee62：cangjie/extractor 是内部经验抽取角色，被错派通用任务会静默
    // 返 [] 让玄女误以为搞定。spawn 入口应拒这两个 role，引导派 luban/pusong。
    #[test]
    fn internal_only_roles_rejected_at_spawn_entry() {
        assert!(is_internal_only_role("cangjie"));
        assert!(is_internal_only_role("extractor"));
        assert!(is_internal_only_role("  cangjie  "), "trim 后仍命中");
        assert!(!is_internal_only_role("luban"));
        assert!(!is_internal_only_role("pusong"));
        assert!(!is_internal_only_role("xuannv"));
    }

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

    /// codex 不是延期 adapter：ROLE.md 写 `metadata.cli: codex` 时，daemon 应走
    /// `WorkerKind::Codex` 分支并把 codex worker 登记进 Fuxi。CodexAgent 是懒
    /// spawn；这里验证控制面能 spawn 成功，同时不 fork 真 codex 子进程。
    #[tokio::test]
    async fn spawn_by_role_with_codex_metadata_registers_codex_worker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let role_dir = dir.path().join("luban-codex");
        std::fs::create_dir_all(&role_dir).expect("role dir");
        std::fs::write(
            role_dir.join("ROLE.md"),
            "---\nname: luban-codex\ndescription: codex test role\nmetadata:\n  cli: codex\n---\n# codex role prompt\n",
        )
        .expect("write role");

        unsafe {
            std::env::set_var("FUXI_ROLES_DIR", dir.path());
        }

        let (fuxi, _bus, _store, _keeper, _oracle) = mock_daemon_parts().await;
        let id = spawn_by_role(
            &fuxi,
            "luban-codex",
            None,
            Some("local".into()),
            None,
            RecallHandle::default(),
            None,
            None,
        )
        .await
        .expect("codex role should spawn");

        unsafe {
            std::env::remove_var("FUXI_ROLES_DIR");
        }

        let cards = fuxi.list_workers().await;
        let card = cards
            .iter()
            .find(|card| card.id == id)
            .expect("spawned worker card");
        assert_eq!(card.profile.role, "luban-codex");
        assert_eq!(card.profile.cli, "codex");
        assert_eq!(card.endpoint, "pid:unspawned");
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

    #[test]
    fn build_dist_gateway_config_reads_metadata() {
        let metadata = serde_json::json!({
            "dist_controller": "https://home.qmledmq.cn",
            "dist_node": "home",
            "dist_token": "t-123",
            "dist_poll_ms": 250
        });
        let cfg = build_dist_gateway_config(&metadata, None)
            .expect("ok")
            .expect("some");
        assert_eq!(cfg.controller, "https://home.qmledmq.cn");
        assert_eq!(cfg.node_id, "home");
        assert_eq!(cfg.token, "t-123");
        assert_eq!(cfg.poll_ms, 250);
    }

    #[test]
    fn build_dist_gateway_config_returns_none_without_inputs() {
        let metadata = serde_json::json!({});
        unsafe {
            std::env::remove_var("FUXI_DIST_CONTROLLER");
            std::env::remove_var("FUXI_DIST_NODE");
            std::env::remove_var(crate::dist::DIST_TOKEN_ENV);
        }
        let cfg = build_dist_gateway_config(&metadata, None).expect("ok");
        assert!(cfg.is_none());
    }

    #[test]
    fn build_dist_gateway_config_node_override_wins_over_metadata_node() {
        let metadata = serde_json::json!({
            "dist_controller": "https://home.qmledmq.cn",
            "dist_node": "other",
            "dist_token": "t-123"
        });
        let cfg = build_dist_gateway_config(&metadata, Some("home"))
            .expect("ok")
            .expect("some");
        assert_eq!(cfg.controller, "https://home.qmledmq.cn");
        assert_eq!(cfg.node_id, "home");
        assert_eq!(cfg.token, "t-123");
    }

    /// Phase 3d: role frontmatter 的 `required_tags: ["codex"]` 数组要被读入
    /// DistGatewayConfig；CLI `--node` 明确给 → 变成 pinned_node。
    #[test]
    fn build_dist_gateway_config_reads_required_tags_from_metadata() {
        let metadata = serde_json::json!({
            "dist_controller": "https://home.qmledmq.cn",
            "dist_node": "home",
            "dist_token": "t-123",
            "required_tags": ["codex", "gpu"]
        });
        let cfg = build_dist_gateway_config(&metadata, None)
            .expect("ok")
            .expect("some");
        assert_eq!(cfg.required_tags, vec!["codex".to_string(), "gpu".into()]);
        // metadata.dist_node 给了明确值，就是 pin
        assert_eq!(cfg.pinned_node.as_deref(), Some("home"));
    }

    /// 缺字段 → 空 required_tags（派工回落到"任一 idle worker"）。
    #[test]
    fn build_dist_gateway_config_missing_required_tags_is_empty() {
        let metadata = serde_json::json!({
            "dist_controller": "https://home.qmledmq.cn",
            "dist_node": "home",
            "dist_token": "t-123",
        });
        let cfg = build_dist_gateway_config(&metadata, None)
            .expect("ok")
            .expect("some");
        assert!(cfg.required_tags.is_empty());
    }

    /// CLI `--node` 显式给 → 覆写 metadata.dist_node 的 pin 目标（同时照搬
    /// required_tags——pin 和 tag filter 两条独立维度）。
    #[test]
    fn build_dist_gateway_config_cli_node_pins_and_keeps_tags() {
        let metadata = serde_json::json!({
            "dist_controller": "https://home.qmledmq.cn",
            "dist_node": "other",
            "dist_token": "t-123",
            "required_tags": ["codex"]
        });
        let cfg = build_dist_gateway_config(&metadata, Some("laptop"))
            .expect("ok")
            .expect("some");
        assert_eq!(cfg.pinned_node.as_deref(), Some("laptop"));
        assert_eq!(cfg.required_tags, vec!["codex".to_string()]);
    }

    // NB: env-only pin 行为（FUXI_DIST_NODE 不 pin）不写单测——CLAUDE.md 标记
    // "env 测试注意"：std::env::set_var 多线程不安全，和其他 test 并行会脏读，
    // 已经因此挂过一次。语义靠 `build_dist_gateway_config` 的实现审查 + 集成
    // smoke 兜底。

    /// Decision 21 phase 3 tag-aware spawn · 给定 project slug → tag 注入。
    #[test]
    fn apply_project_tag_appends_when_slug_present() {
        let mut tags = vec!["codex".into()];
        apply_project_tag_to_required_tags(&mut tags, Some("erp"));
        assert_eq!(tags, vec!["codex".to_string(), "project:erp".to_string()]);
    }

    /// project=None / 空串 / 全空白 → 不动 tags。
    #[test]
    fn apply_project_tag_noop_when_slug_missing_or_blank() {
        let mut tags = vec!["codex".into()];
        apply_project_tag_to_required_tags(&mut tags, None);
        apply_project_tag_to_required_tags(&mut tags, Some(""));
        apply_project_tag_to_required_tags(&mut tags, Some("   "));
        assert_eq!(tags, vec!["codex".to_string()]);
    }

    /// 已有同 tag → 幂等不重复添加。
    #[test]
    fn apply_project_tag_is_idempotent() {
        let mut tags = vec!["project:erp".into(), "codex".into()];
        apply_project_tag_to_required_tags(&mut tags, Some("erp"));
        assert_eq!(
            tags,
            vec!["project:erp".to_string(), "codex".to_string()],
            "重复调用应不再添加"
        );
    }

    #[test]
    fn normalize_cli_rejects_unknown() {
        let err = normalize_cli(Some("foo".into())).expect_err("invalid cli should fail");
        assert!(err.to_string().contains("未知 CLI 覆写"));
    }

    #[test]
    fn normalize_node_trims_and_drops_empty() {
        assert_eq!(
            normalize_node(Some(" home ".into())).as_deref(),
            Some("home")
        );
        assert!(normalize_node(Some("   ".into())).is_none());
    }

    #[test]
    fn node_scoped_dist_token_env_normalizes_key() {
        assert_eq!(
            node_scoped_dist_token_env("home-prod").as_deref(),
            Some("FUXI_DIST_HOME_PROD_TOKEN")
        );
    }

    fn mk_chunk(kind: crate::dist::ProgressKind, text: &str) -> crate::dist::ProgressChunk {
        crate::dist::ProgressChunk {
            seq: 1,
            kind,
            text: text.into(),
        }
    }

    #[test]
    fn progress_chunk_assistant_text_is_raw() {
        let ev = progress_chunk_to_event_kind(
            &mk_chunk(crate::dist::ProgressKind::AssistantText, "hello world"),
            AgentId::new(),
            TaskId::new(),
        );
        let EventKind::AgentResponded { text, .. } = ev else {
            panic!("expected AgentResponded");
        };
        assert_eq!(text, "hello world");
    }

    #[test]
    fn progress_chunk_non_assistant_kinds_use_prefixes() {
        let cases = [
            (crate::dist::ProgressKind::Thinking, "[thinking] "),
            (crate::dist::ProgressKind::ToolCall, "[tool] "),
            (crate::dist::ProgressKind::Error, "[error] "),
        ];
        let aid = AgentId::new();
        let tid = TaskId::new();
        for (kind, prefix) in cases {
            let ev = progress_chunk_to_event_kind(&mk_chunk(kind, "x"), aid, tid);
            let EventKind::AgentResponded { text, .. } = ev else {
                panic!("expected AgentResponded");
            };
            assert!(text.starts_with(prefix), "{:?}: got {}", kind, text);
        }
    }

    /// issue 18dfccea：跨节点门客发的 `_fuxi:request_review` sentinel 走 dist
    /// gateway 路径——这里收到的是远端透传的 `ProgressKind::AssistantText`，不经
    /// 本地 cc parser。若 `progress_chunk_to_event_kind` 不识别 sentinel，sentinel
    /// 被当普通 `AgentResponded` 透传 → bridge 白名单不含 AgentResponded → 玄女
    /// 永远收不到 `[REVIEW_REQUEST]`。修复后此路径必须翻成 `AgentRequestReview`。
    #[test]
    fn progress_chunk_assistant_text_sentinel_becomes_request_review() {
        let aid = AgentId::new();
        let tid = TaskId::new();
        let sentinel = r#"{"_fuxi":"request_review","kind":"code_change","summary":"重构完工待审","artifact_ref":"sha:abc"}"#;
        let ev = progress_chunk_to_event_kind(
            &mk_chunk(crate::dist::ProgressKind::AssistantText, sentinel),
            aid,
            tid,
        );
        let EventKind::AgentRequestReview {
            agent,
            task,
            deliverable_kind,
            summary,
            artifact_ref,
        } = ev
        else {
            panic!("sentinel chunk 应翻成 AgentRequestReview，got {ev:?}");
        };
        assert_eq!(agent, aid);
        assert_eq!(task, tid);
        assert_eq!(
            deliverable_kind,
            fuxi_core::event::DeliverableKind::CodeChange
        );
        assert_eq!(summary, "重构完工待审");
        assert_eq!(artifact_ref.as_deref(), Some("sha:abc"));
    }

    /// 防误触：非 sentinel 的普通 AssistantText（含碰巧以 `{` 开头的 JSON）
    /// 仍走 `AgentResponded`——sentinel 识别必须严格，不能误吞门客正常输出。
    #[test]
    fn progress_chunk_assistant_text_non_sentinel_stays_responded() {
        let aid = AgentId::new();
        let tid = TaskId::new();
        let ev = progress_chunk_to_event_kind(
            &mk_chunk(
                crate::dist::ProgressKind::AssistantText,
                r#"{"some":"normal json"}"#,
            ),
            aid,
            tid,
        );
        assert!(
            matches!(ev, EventKind::AgentResponded { .. }),
            "非 sentinel JSON 不该翻成 AgentRequestReview，got {ev:?}"
        );
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

    /// 块5：捕获被 dispatch 的 task，验 daemon 把 Command::Dispatch.topic_id 串进
    /// task.topic_id。
    struct TopicCapturingAgent {
        card: fuxi_core::agent::AgentCard,
        last: std::sync::Arc<tokio::sync::Mutex<Option<fuxi_core::task::Task>>>,
    }
    impl TopicCapturingAgent {
        fn new() -> (
            std::sync::Arc<Self>,
            std::sync::Arc<tokio::sync::Mutex<Option<fuxi_core::task::Task>>>,
        ) {
            let slot = std::sync::Arc::new(tokio::sync::Mutex::new(None));
            let card = fuxi_core::agent::AgentCard {
                id: AgentId::new(),
                profile: fuxi_core::agent::AgentProfile {
                    name: "cap".into(),
                    role: "dev".into(),
                    cli: "stub".into(),
                    system_prompt: String::new(),
                    tags: vec![],
                    extra: Default::default(),
                },
                endpoint: "stub://cap".into(),
                status: fuxi_core::agent::AgentStatus::Idle,
            };
            (
                std::sync::Arc::new(Self {
                    card,
                    last: slot.clone(),
                }),
                slot,
            )
        }
    }
    #[async_trait::async_trait]
    impl fuxi_core::agent::Agent for TopicCapturingAgent {
        fn card(&self) -> &fuxi_core::agent::AgentCard {
            &self.card
        }
        async fn dispatch(
            &self,
            task: fuxi_core::task::Task,
        ) -> fuxi_core::Result<tokio::sync::mpsc::Receiver<fuxi_core::Event>> {
            *self.last.lock().await = Some(task);
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
        async fn send_message(&self, _t: fuxi_core::id::TaskId, _x: &str) -> fuxi_core::Result<()> {
            Ok(())
        }
        async fn cancel(&self, _t: fuxi_core::id::TaskId) -> fuxi_core::Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> fuxi_core::Result<()> {
            Ok(())
        }
    }

    /// 块5 步7.5：Command::Dispatch{topic_id: Some(uuid)} → 被派 task.topic_id 命中该 topic。
    #[tokio::test]
    async fn dispatch_stamps_topic_id_from_command() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let (agent, slot) = TopicCapturingAgent::new();
        let id = fuxi.insert_agent(agent, None).await;
        let topic = fuxi_core::TopicId::new();

        let _ = dispatch_command(
            fuxi.clone(),
            bus,
            store,
            keeper,
            oracle,
            None,
            Command::Dispatch {
                agent_id: id.to_string(),
                task_id: None,
                title: "t".into(),
                body: Some("do x".into()),
                pinned_node: None,
                required_tags: vec![],
                topic_id: Some(topic.as_uuid().to_string()),
            },
            Arc::new(Notify::new()),
        )
        .await;

        // dispatch pump 异步——给点时间让 agent.dispatch 收到 task。
        for _ in 0..50 {
            if slot.lock().await.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let task = slot.lock().await.clone().expect("agent 应收到 task");
        assert_eq!(
            task.topic_id,
            Some(topic),
            "daemon 应把 --topic 串进 task.topic_id"
        );
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
            None,
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
            None,
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

    /// `Command::Nodes` 在 dist controller 缺席时返 Err（不是 panic）——
    /// 玄女工具拿到具体 message 能给用户回"忘了开 --dist-token"。
    #[tokio::test]
    async fn nodes_without_dist_controller_returns_err() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let resp = dispatch_command(
            fuxi,
            bus,
            store,
            keeper,
            oracle,
            None,
            Command::Nodes,
            Arc::new(Notify::new()),
        )
        .await;
        match resp {
            Response::Err { error } => {
                assert!(error.contains("dist controller 未启用"), "got: {error}")
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    /// 接好 dist controller 后 `Command::Nodes` 返 nodes 数组——内容来自
    /// `nodes_snapshot`，daemon 不再加工。register 一个 worker 后应能在响应里看到。
    #[tokio::test]
    async fn nodes_with_dist_controller_returns_snapshot() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let ctrl = Arc::new(crate::dist::DistController::new("tok".into(), bus.clone()));
        ctrl.register("home".into(), vec!["cc".into()], 2).await;

        let resp = dispatch_command(
            fuxi,
            bus,
            store,
            keeper,
            oracle,
            Some(ctrl),
            Command::Nodes,
            Arc::new(Notify::new()),
        )
        .await;
        match resp {
            Response::Ok { data } => {
                let nodes = data.get("nodes").expect("nodes 字段").as_array().unwrap();
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0]["node_id"], "home");
                assert_eq!(nodes[0]["status"], "alive");
                assert_eq!(nodes[0]["max_concurrency"], 2);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// `Daemon::new_for_im_start` 一次性把 dist controller 句柄挂上——`fuxi im start`
    /// 路径用它替代 `Daemon::new(...).with_dist(...)`，required arg 编译期防漏。
    /// 反演 commit 之前 `im.rs:521-527` 漏链 `with_dist` 导致 `fuxi nodes` 误报的 bug。
    #[tokio::test]
    async fn new_for_im_start_attaches_dist_ctrl() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let ctrl = Arc::new(crate::dist::DistController::new("tok".into(), bus.clone()));
        ctrl.register("home".into(), vec!["cc".into()], 2).await;

        let daemon = Daemon::new_for_im_start(fuxi, bus, store, keeper, oracle, ctrl.clone());
        assert!(
            daemon.dist.is_some(),
            "new_for_im_start 必须把 dist 写进字段——这是 im.rs daemon wiring 的硬保证"
        );
    }

    /// 通过 `new_for_im_start` 构造的 daemon 调 `Command::Nodes` 应返 Ok——
    /// 端到端验证 im-start 路径上的 `fuxi nodes` IPC 真能拿到 nodes snapshot
    /// （之前 daemon.dist=None 报"未启用"）。
    #[tokio::test]
    async fn nodes_via_new_for_im_start_returns_snapshot() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let ctrl = Arc::new(crate::dist::DistController::new("tok".into(), bus.clone()));
        ctrl.register("home".into(), vec!["cc".into()], 2).await;
        let daemon = Daemon::new_for_im_start(
            fuxi.clone(),
            bus.clone(),
            store.clone(),
            keeper.clone(),
            oracle.clone(),
            ctrl.clone(),
        );
        let dist_opt = daemon.dist.clone();

        let resp = dispatch_command(
            fuxi,
            bus,
            store,
            keeper,
            oracle,
            dist_opt,
            Command::Nodes,
            Arc::new(Notify::new()),
        )
        .await;
        match resp {
            Response::Ok { data } => {
                let nodes = data.get("nodes").expect("nodes 字段").as_array().unwrap();
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0]["node_id"], "home");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    /// PR-C 反演 issue #5b7c99a7：dispatch `--pinned-node` 拿到 controller registry
    /// 里没有的 node_id（如玄女把 `mac-local` 当了 alias），daemon 必须立刻 4xx
    /// 拒绝，而不是把 task 入队让 worker 永远拉不到。
    #[tokio::test]
    async fn dispatch_with_unknown_pinned_node_returns_err() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let ctrl = Arc::new(crate::dist::DistController::new("tok".into(), bus.clone()));
        ctrl.register("real-node".into(), vec![], 1).await;

        let resp = dispatch_command(
            fuxi,
            bus,
            store,
            keeper,
            oracle,
            Some(ctrl),
            Command::Dispatch {
                agent_id: uuid::Uuid::new_v4().to_string(),
                task_id: None,
                title: "test".into(),
                body: Some("echo hi".into()),
                pinned_node: Some("ghost-node".into()),
                required_tags: vec![],
                topic_id: None,
            },
            Arc::new(Notify::new()),
        )
        .await;
        match resp {
            Response::Err { error } => {
                assert!(
                    error.contains("pinned_node") && error.contains("ghost-node"),
                    "error 应明示 pinned_node 名字让玄女能纠正；got: {error}"
                );
            }
            other => panic!("expected Err for unknown pinned_node, got {other:?}"),
        }
    }

    /// PR-C 配套：pinned_node 是 controller 已知的 alive 节点 → 不被 pinned_node
    /// 校验挡住，继续走 dispatch 路径（最终因 mock agent 不在 shelf 上失败，
    /// 但**那是别的错**——本测试只确认 pinned_node 校验放行）。
    #[tokio::test]
    async fn dispatch_with_known_pinned_node_passes_validation() {
        let (fuxi, bus, store, keeper, oracle) = mock_daemon_parts().await;
        let ctrl = Arc::new(crate::dist::DistController::new("tok".into(), bus.clone()));
        ctrl.register("real-node".into(), vec![], 1).await;

        let resp = dispatch_command(
            fuxi,
            bus,
            store,
            keeper,
            oracle,
            Some(ctrl),
            Command::Dispatch {
                agent_id: uuid::Uuid::new_v4().to_string(),
                task_id: None,
                title: "test".into(),
                body: Some("echo hi".into()),
                pinned_node: Some("real-node".into()),
                required_tags: vec![],
                topic_id: None,
            },
            Arc::new(Notify::new()),
        )
        .await;
        // 期望：要么 Ok（如果 mock fuxi 接得住 dispatch），要么 Err 但**不含**
        // pinned_node 字样（pinned_node 校验已放行，挂在别的链路）。
        if let Response::Err { error } = resp {
            assert!(
                !error.contains("no alive node matches"),
                "pinned_node 校验不该拦真节点；got: {error}"
            );
        }
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
            None,
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
