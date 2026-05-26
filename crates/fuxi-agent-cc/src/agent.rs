//! `CcAgent`——实现 `fuxi_core::Agent` 的 Claude Code 门客。
//!
//! **v0.1 薄片 H 开始走 WS 反连**（`--sdk-url ws://127.0.0.1:<port>/ws/cli/<sid>`）。
//! 老的 stdio `--print` 路径被替换——但 `parser::parse_line` + `translate` 仍
//! 复用（NDJSON wire 格式没变，只是传输层从 stdin/stdout 换到 WS）。
//!
//! 生命周期：
//!
//! 1. `launch_with_id` 并发起：
//!    - `WsChannel::bind(sid)` 起本地 axum server → 拿到 port；
//!    - `spawn_claude(cfg with --sdk-url)` 启动 CLI，CLI 反连回来；
//!    - `wait_connect(30s)` 等 CLI 到达；
//!    - 起 "message pump" task 不断 `channel.recv()` → parse + translate → 派发事件。
//! 2. `dispatch(task)` 取 `cli_session_id`（可能刚收到的 `system/init`）、
//!    注册新的 event_tx、通过 WS 发 `{type:"user",...}`，返回 Receiver。
//! 3. `send_message` 复用同一个 WS channel 发 user message——**追加式介入**
//!    （门客下一 turn 看见）。
//! 4. `cancel` 走 WS `control_request { subtype: "interrupt" }` 打断当前 turn
//!    ——**打断式介入**（不再像 stdio 时代靠 SIGINT）。
//! 5. `shutdown` drop WsChannel（server 停）+ SIGTERM → 5s → SIGKILL。

use crate::config::CcLaunchConfig;
use crate::parser::{self, TranslateState, parse_line, translate};
use crate::pending::{EnqueueOutcome, PendingOutbox};
use crate::spawn::{SpawnedCc, spawn_claude};
use crate::ws_bridge::WsChannel;
use async_trait::async_trait;
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::{DeliverableKind, Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::Task;
use fuxi_core::{CoreError, Result};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::{Mutex, mpsc};

/// Agent 事件 channel 的默认缓冲——Firehose 应当快速消费；
/// 32 足以吸收 cc 的爆发帧。
const EVENT_CHANNEL_BUFFER: usize = 32;

/// 默认等待 CLI 反连的超时——anya 实测 3-10 秒够用，留 30s 给首次启动
/// （keychain 拉取 / 模型预热）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Bug A 玄女堵塞自愈 · cancel 后仍卡 `Stopping` 多久强制摊回 Idle。
///
/// 30s 是踩在"用户感知卡顿但还能忍" + "正常 turn 中断 ack 应在数秒内回到"
/// 之间的折中——home node 实测从 cancel 到 cc 真停 turn 通常 ≤2s；30s 足够
/// 覆盖 cc 在长 thinking/tool-loop 中段的最坏 ack 延迟。再长用户体感"假死"。
const STOPPING_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);

/// `fuxi-agent-cc` 的错误类型——聚合 io / serde / core / ws。
#[derive(Debug, thiserror::Error)]
pub enum CcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("core: {0}")]
    Core(#[from] CoreError),
    #[error("ws: {0}")]
    Ws(#[from] crate::ws_bridge::WsError),
    #[error("cc process exited before WS connected")]
    EarlyExit,
    #[error("{0}")]
    Other(String),
}

impl From<CcError> for CoreError {
    fn from(err: CcError) -> Self {
        match err {
            CcError::Io(e) => CoreError::Io(e),
            CcError::Serde(e) => CoreError::Serde(e),
            CcError::Core(e) => e,
            other => CoreError::Other(other.to_string()),
        }
    }
}

/// cc 子进程和 WS 通道的共享内部状态。
struct Inner {
    channel: Arc<WsChannel>,
    child: Option<Child>,
    status: AgentStatus,
    /// 当前 dispatch 的事件管道。dispatch 设置，shutdown / drop 时清空。
    active_tx: Option<mpsc::Sender<Event>>,
    /// 当前活跃任务 id——写进事件 meta。
    current_task: Option<TaskId>,
    /// Phase 1 topic 路由：当前活跃 task 关联的 topic_id（来自 `Task.topic_id`）。
    /// dispatch 写入，emit 时 → `meta.topic_id`，让 SystemEventBridge filter 决定
    /// 是否注入当前玄女（跨 topic 的 worker event 不污染当前 prompt）。`None` =
    /// 视作默认 topic [`fuxi_core::TopicId::general`]（兼容老 dispatch 路径）。
    current_topic: Option<fuxi_core::TopicId>,
    translate_state: TranslateState,
    /// 死亡信号发送端。
    /// pump_loop 退出（WS channel 关闭）时发一条"ws closed"；shutdown 主动清空。
    /// why `Option`: 一旦发过一次就置 None，避免重复发。
    death_tx: Option<mpsc::UnboundedSender<String>>,
    /// M2.1 · 消息黑洞修：cc 在 tool loop 中不 poll WS，busy 时直送会被吞。
    /// `send_message` 总是入队，pump 在 turn terminal（`ResultSuccess`/`ResultError`）
    /// 处 drain 后按 FIFO 调 `channel.send`；idle 时由 `send_message` 立即 drain
    /// 自送（无 turn 触发它）。
    pending: PendingOutbox,
    /// Bug A · 玄女堵塞自愈：cancel() 发 interrupt 时记下 request_id；pump_loop
    /// 收 `control_response` 时如果 ack 的 request_id 命中、subtype="success" → 触发
    /// 同 turn-terminal 同款 drain（status 摊 Idle + drain pending）。
    /// 不命中（不是 interrupt ack）则继续走老逻辑日志即丢。
    last_interrupt_request_id: Option<String>,
    /// Bug A · cancel 代际：每次 cancel 自增 1。30s watchdog 携带 spawn 时的
    /// 代际值，醒来比对当前——大于自己 → 已被新 cancel 取代，silent return；
    /// 否则强制摊回 Idle + 发 `AgentInterrupted{reason:"stopping_timeout"}` 警告
    /// 玄女自检。WHY counter 不 channel：watchdog spawn 完不持锁；用 `Inner.lock()`
    /// 醒来比对一次原子读即可，比 watch 简单。
    cancel_generation: u64,
}

/// Claude Code 门客——WS 反连承载。
pub struct CcAgent {
    card: AgentCard,
    inner: Arc<Mutex<Inner>>,
    /// 死亡信号接收端——被 `take_death_watch` 拿走后就是 None。
    /// why 用 `std::sync::Mutex` 而非 tokio：take_death_watch 是同步 API，
    /// 锁内只 std::mem::take 一瞬，不需要 async。
    death_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    /// 保活 pump task 句柄。drop 不取消（channel drop 自会带停）。
    _pump: tokio::task::JoinHandle<()>,
}

impl CcAgent {
    /// WS 反连模式下 launch 必须 async（要 await WS 反连）。
    pub async fn launch(profile: AgentProfile, cfg: CcLaunchConfig) -> Result<Self> {
        Self::launch_with_id(AgentId::new(), profile, cfg).await
    }

    /// 编排层指定 id——解决 S1 教训的 AgentId 双生问题。
    pub async fn launch_with_id(
        id: AgentId,
        profile: AgentProfile,
        mut cfg: CcLaunchConfig,
    ) -> Result<Self> {
        // role 先 clone 一份给 TranslateState——profile 后面会被 move 进 AgentCard。
        // parser sentinel 路径要按 role 决定是否参与（cangjie/extractor 不参与，
        // 防 cangjie 复读 trajectory 里的 sentinel 样本触发死循环）。
        let role_for_parser = profile.role.clone();
        // 1. WS server 先起来拿到 port
        let sid = uuid::Uuid::new_v4().to_string();
        let channel = Arc::new(WsChannel::bind(&sid).await.map_err(CcError::from)?);
        cfg.sdk_url = Some(channel.url());

        // 2. 起 claude——加 `--sdk-url` 的 argv 已由 config 处理
        let SpawnedCc {
            mut child,
            stdin: _,  // SDK 模式下不用（`-p ""` 已占位）——但要 drop 防 FD 泄漏
            stdout: _, // 同上
            pid,
        } = spawn_claude(&cfg).map_err(CcError::Io)?;

        // 3. 等 CLI 反连；early-exit 时尽早失败
        //
        // 用 block 限制 future 的 borrow 作用域——select 之后我们还要把 child 塞进
        // Inner。如果不 scope 住 `child.wait()` 这个 future，它会持续借走 child
        // 到函数末尾。
        enum ConnectOutcome {
            Connected,
            Exited(std::io::Result<std::process::ExitStatus>),
            Failed(crate::ws_bridge::WsError),
        }
        let outcome = {
            let connect_fut = channel.wait_connect(CONNECT_TIMEOUT);
            let exit_fut = child.wait();
            tokio::pin!(connect_fut);
            tokio::pin!(exit_fut);
            tokio::select! {
                biased;
                r = &mut connect_fut => match r {
                    Ok(()) => ConnectOutcome::Connected,
                    Err(e) => ConnectOutcome::Failed(e),
                },
                r = &mut exit_fut => ConnectOutcome::Exited(r),
            }
        };
        match outcome {
            ConnectOutcome::Connected => {}
            ConnectOutcome::Failed(e) => return Err(CcError::from(e).into()),
            ConnectOutcome::Exited(r) => {
                match r {
                    Ok(status) => tracing::error!(?status, "cc exited before WS connected"),
                    Err(e) => tracing::error!(error = %e, "cc wait() errored before WS connected"),
                }
                return Err(CcError::EarlyExit.into());
            }
        }

        tracing::info!(%id, port = channel.port, "cc agent connected via WS");

        let card = AgentCard {
            id,
            profile,
            endpoint: match pid {
                Some(p) => format!("pid:{p}"),
                None => "pid:unknown".to_string(),
            },
            status: AgentStatus::Idle,
        };

        // death signal wiring
        let (death_tx, death_rx) = mpsc::unbounded_channel::<String>();

        let inner = Arc::new(Mutex::new(Inner {
            channel: channel.clone(),
            child: Some(child),
            status: AgentStatus::Idle,
            active_tx: None,
            current_task: None,
            current_topic: None,
            translate_state: TranslateState::with_role(role_for_parser.clone())
                .with_window_size(crate::config::resolve_default_window_size()),
            death_tx: Some(death_tx),
            pending: PendingOutbox::new(),
            last_interrupt_request_id: None,
            cancel_generation: 0,
        }));

        // 4. pump task——长跑，直到 channel 关闭
        let pump = tokio::spawn(pump_loop(channel.clone(), id, pid, inner.clone()));

        Ok(Self {
            card,
            inner,
            death_rx: std::sync::Mutex::new(Some(death_rx)),
            _pump: pump,
        })
    }

    /// 让编排层接管死亡信号的接收端。
    ///
    /// 设计理由：`CcAgent` 不直接依赖 EventBus——保持 adapter 层与事件层解耦；
    /// Fuxi 从这里拿到 rx 后，spawn 一个 task 监听 → publish `AgentDead`。
    ///
    /// 返回 `None` 表示已被取走（每个 CcAgent 一生只能 take 一次）。
    pub fn take_death_watch(&self) -> Option<mpsc::UnboundedReceiver<String>> {
        self.death_rx
            .lock()
            .ok()
            .and_then(|mut g| std::mem::take(&mut *g))
    }

    /// Bug A 测试 fixture · 起 [`WsChannel`] 但**不**起真 cc 子进程，让测试自己用
    /// `tokio_tungstenite::connect_async(url)` 反连模拟 cc。返回 `(agent, url)`。
    ///
    /// 跟 `launch_with_id` 唯一差别：跳过 `spawn_claude` + `wait_connect`，直接
    /// 起 pump_loop。pump_loop 内 `channel.recv().await` 在没人反连时只是 await，
    /// 等测试反连后 ws_handler 把消息推进 incoming queue 自然继续。
    ///
    /// `#[doc(hidden)]` 暴露——生产路径不该调，但 integration test
    /// （`tests/cancel_drain.rs` 等）需要 always-pub。改 `#[cfg(test)]` 会让
    /// integration test 编译不通过（cfg(test) 只覆盖 lib 内单测）。
    #[doc(hidden)]
    pub async fn for_test_with_fake_cli(profile: AgentProfile) -> Result<(Self, String)> {
        let id = AgentId::new();
        let role_for_parser = profile.role.clone();
        let sid = uuid::Uuid::new_v4().to_string();
        let channel = Arc::new(WsChannel::bind(&sid).await.map_err(CcError::from)?);
        let url = channel.url();

        let card = AgentCard {
            id,
            profile,
            endpoint: "test:fake-cli".to_string(),
            status: AgentStatus::Idle,
        };

        let (death_tx, death_rx) = mpsc::unbounded_channel::<String>();
        let inner = Arc::new(Mutex::new(Inner {
            channel: channel.clone(),
            child: None,
            status: AgentStatus::Idle,
            active_tx: None,
            current_task: None,
            current_topic: None,
            translate_state: TranslateState::with_role(role_for_parser)
                .with_window_size(crate::config::resolve_default_window_size()),
            death_tx: Some(death_tx),
            pending: PendingOutbox::new(),
            last_interrupt_request_id: None,
            cancel_generation: 0,
        }));

        let pump = tokio::spawn(pump_loop(channel.clone(), id, None, inner.clone()));

        Ok((
            Self {
                card,
                inner,
                death_rx: std::sync::Mutex::new(Some(death_rx)),
                _pump: pump,
            },
            url,
        ))
    }

    /// Bug A 测试 helper · 读当前 status，绕过外部调用语义只读内部字段。
    #[doc(hidden)]
    pub async fn test_status(&self) -> AgentStatus {
        self.inner.lock().await.status
    }

    /// Bug A 测试 helper · pending 队列剩余条数。
    #[doc(hidden)]
    pub async fn test_pending_len(&self) -> usize {
        self.inner.lock().await.pending.len()
    }

    /// Bug A 测试 helper · 上次 cancel 用的 control_request id。`None` 即从未 cancel。
    #[doc(hidden)]
    pub async fn test_last_interrupt_request_id(&self) -> Option<String> {
        self.inner.lock().await.last_interrupt_request_id.clone()
    }

    /// Bug A 测试 helper · cancel 代际（每次 cancel +1）。
    #[doc(hidden)]
    pub async fn test_cancel_generation(&self) -> u64 {
        self.inner.lock().await.cancel_generation
    }

    /// 组装 `{type:"user", ...}` JSON。
    fn user_message_value(text: &str, session_id: &str) -> Value {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type":"text", "text": text}]
            },
            "parent_tool_use_id": Value::Null,
            "session_id": session_id,
        })
    }
}

#[async_trait]
impl Agent for CcAgent {
    fn card(&self) -> &AgentCard {
        &self.card
    }

    async fn dispatch(&self, task: Task) -> Result<mpsc::Receiver<Event>> {
        let (tx, rx) = mpsc::channel::<Event>(EVENT_CHANNEL_BUFFER);

        // bug #77：之前 prepend `{title}\n\n{description}` 把内部 task.title（如
        // "user-turn"）灌进 cc prompt → cc 模型 echo 该字面到 reply，用户看见
        // 玄女回复："Human: user-turn\n\n[REVIEW_REQUEST]..."。title 是 dashboard
        // 元数据不该污染 prompt；空 description 时再 fallback title 防空消息。
        let body = if task.description.is_empty() {
            task.title.clone()
        } else {
            task.description.clone()
        };

        // 拿 cli_session_id（可能 None——用空串，cc 会在 init 后关联起来）
        let cli_sid = self
            .inner
            .lock()
            .await
            .channel
            .cli_session_id()
            .await
            .unwrap_or_default();
        let msg = Self::user_message_value(&body, &cli_sid);

        // 注册 active_tx / current_task / 重置翻译状态
        {
            let mut inner = self.inner.lock().await;
            inner.status = AgentStatus::Busy;
            inner.active_tx = Some(tx);
            inner.current_task = Some(task.id);
            inner.current_topic = task.topic_id;
            inner.translate_state = TranslateState::with_role(self.card.profile.role.clone())
                .with_window_size(crate::config::resolve_default_window_size());
            // 发送必须在锁外——channel.send 本身不会 block，但保守点
            inner.channel.send(msg).map_err(CcError::from)?;
        }

        Ok(rx)
    }

    async fn send_message(&self, _task_id: TaskId, text: &str) -> Result<()> {
        // M2.1 · 消息黑洞修：
        // 旧路径直接 channel.send，但 cc 在 tool loop 的 turn 中**不 poll WS**——
        // 消息进了 axum outgoing chan 也送不到 cc，下个 turn 后才被消化。
        // 修法：总是入队，pump 在 turn terminal 处 drain 自送；idle 时由本函数立即 drain。
        //
        // overflow 时驱逐最旧消息 + 通过 active_tx 发 `AgentInterrupted{queue_overflow}`
        // 让玄女知情（active_tx 为 None 时只能 trace warn——CcAgent 不直接持 EventBus，
        // 等下游接通过 take_death_watch 类似的回调注入 bus 才能完全无损上报，TODO M2.x）。
        let cli_sid = self
            .inner
            .lock()
            .await
            .channel
            .cli_session_id()
            .await
            .unwrap_or_default();

        // 入队 + 决定 idle 是否立即 drain；锁内只动状态不做 IO。
        let (to_send, status_snapshot, overflow_event) = {
            let mut inner = self.inner.lock().await;
            let outcome = inner.pending.enqueue(text.to_string());
            let overflow_evt = match outcome {
                EnqueueOutcome::Queued => None,
                EnqueueOutcome::Overflowed { dropped } => {
                    tracing::warn!(
                        agent = %self.card.id,
                        dropped_preview = %dropped.chars().take(80).collect::<String>(),
                        "pending queue overflow，驱逐最旧消息"
                    );
                    let mut meta = EventMeta::now();
                    meta.agent = Some(self.card.id);
                    meta.task = inner.current_task;
                    meta.topic_id = inner.current_topic;
                    Some(Event {
                        meta,
                        kind: EventKind::AgentInterrupted {
                            reason: "queue_overflow".to_string(),
                        },
                    })
                }
            };

            // idle → 立即 drain（无 turn 会触发 pump 的 drain）；busy → 等 pump terminal。
            let snapshot = inner.status;
            let drained = if matches!(snapshot, AgentStatus::Idle) {
                inner.pending.drain_all()
            } else {
                Vec::new()
            };
            (drained, snapshot, overflow_evt)
        };

        // overflow 事件：尽力上报到 active_tx（下游 orchestrator 听这个 chan 转
        // EventBus）。若 active_tx 已被清空，只能日志记账——参见 M2.x TODO。
        if let Some(ev) = overflow_event {
            let tx_opt = self.inner.lock().await.active_tx.clone();
            if let Some(tx) = tx_opt {
                if tx.send(ev).await.is_err() {
                    tracing::debug!(agent = %self.card.id, "overflow 事件投递失败：active_tx 已关闭");
                }
            } else {
                tracing::warn!(
                    agent = %self.card.id,
                    "overflow 事件无处投递：active_tx=None；玄女将无法实时获知（下游需补 bus 回调）"
                );
            }
        }

        // 锁外发送——channel.send 是 unbounded mpsc，不会 block；保守起见离锁做。
        if !to_send.is_empty() {
            // 从 inner 里再取一次 channel 引用——不持锁。
            let channel = self.inner.lock().await.channel.clone();
            for body in to_send {
                let msg = Self::user_message_value(&body, &cli_sid);
                channel.send(msg).map_err(CcError::from)?;
            }
            tracing::info!(
                agent = %self.card.id,
                status = ?status_snapshot,
                "追加介入：idle drain 直送"
            );
        } else {
            // why 取 len 拆两步：tracing 宏会借住临时值跨 await，导致 future !Send。
            let pending_len = self.inner.lock().await.pending.len();
            tracing::info!(
                agent = %self.card.id,
                status = ?status_snapshot,
                pending = pending_len,
                "追加介入：busy 入队，等 turn terminal drain"
            );
        }
        Ok(())
    }

    async fn cancel(&self, _task_id: TaskId) -> Result<()> {
        // 打断式介入：WS `control_request { subtype: "interrupt" }`。
        // 参照 anya claude-code-backend.ts:470-485。
        //
        // Bug A · 玄女堵塞自愈：cc 收到 interrupt 后**只 emit `control_response`**，
        // 并不一定继续 emit `ResultSuccess`/`Error`——尤其玄女 thinking/tool-loop
        // 中段被打断时。turn-terminal 是 pump_loop 唯一摊回 Idle 的入口，永远不
        // 来 → status 永远 Stopping → pending 队列只长不 drain → 玄女假死 44 分钟。
        //
        // 治法两条：
        // 1. 记下 `last_interrupt_request_id` 让 pump_loop 收 control_response
        //    success 时直接走 drain（同 turn-terminal 路径）。
        // 2. 30s watchdog 兜底：cc 既不 emit terminal 也不 ack 的极端场景下强制
        //    摊回 + 发 `AgentInterrupted{reason:"stopping_timeout"}` 让玄女自检。
        let request_id = uuid::Uuid::new_v4().to_string();
        let msg = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": {"subtype": "interrupt"},
        });
        let (current_task, current_topic, my_generation) = {
            let mut inner = self.inner.lock().await;
            inner.channel.send(msg).map_err(CcError::from)?;
            inner.status = AgentStatus::Stopping;
            inner.last_interrupt_request_id = Some(request_id.clone());
            inner.cancel_generation = inner.cancel_generation.wrapping_add(1);
            (
                inner.current_task,
                inner.current_topic,
                inner.cancel_generation,
            )
        };
        tracing::info!(
            agent = %self.card.id,
            request_id,
            generation = my_generation,
            "打断介入：sent control_request interrupt"
        );

        // Bug A · 30s watchdog——fire-and-forget，generation counter 取消语义。
        // 新 cancel 把 generation+1 → 旧 watchdog 醒来发现 inner.cancel_generation
        // 已超自己 → silent return；当前最新那个 watchdog 跑完自然摊回。
        let inner_cl = self.inner.clone();
        let agent_id = self.card.id;
        tokio::spawn(async move {
            tokio::time::sleep(STOPPING_WATCHDOG_TIMEOUT).await;
            drain_pending_to_idle(
                &inner_cl,
                agent_id,
                current_task,
                current_topic,
                DrainCause::WatchdogTimeout {
                    expected_generation: my_generation,
                },
            )
            .await;
        });
        Ok(())
    }

    async fn session_id(&self) -> Option<String> {
        // P2 召回：cc 的 cli_session_id 在 CLI 推 system/init 后才填好；
        // dispatch pump 在 task Done 时来取，那时一定已经填了——任 None
        // 也是合法（譬如门客在 init 前就死掉），上层 silent skip。
        self.inner.lock().await.channel.cli_session_id().await
    }

    /// 程序化 nudge 入口（Decision 13）。主路径是 LLM 输出 sentinel JSON 经
    /// parser 翻 `AgentRequestReview`——本方法供编排层 / 测试 / 未来工具
    /// 从外部主动让 cc 门客发 review。
    async fn request_review(
        &self,
        task_id: TaskId,
        deliverable_kind: DeliverableKind,
        summary: String,
        artifact_ref: Option<String>,
    ) -> Result<()> {
        // Phase 1：current_topic 来自 dispatch 时设的值；review nudge 跟它来源 task
        // 同 topic，让 SystemEventBridge filter 判定正确。
        let topic_id = self.inner.lock().await.current_topic;
        let event = build_review_event(
            self.card.id,
            task_id,
            topic_id,
            deliverable_kind,
            summary,
            artifact_ref,
        );

        // 仿 send_message overflow 路径：clone tx 再锁外发，`active_tx=None`
        // 时 trace warn——cc 在 dispatch 之前 / shutdown 之后没 active_tx。
        let tx_opt = self.inner.lock().await.active_tx.clone();
        if let Some(tx) = tx_opt {
            if tx.send(event).await.is_err() {
                tracing::debug!(agent = %self.card.id, "request_review 投递失败：active_tx 已关闭");
                return Err(CoreError::Other(
                    "request_review delivery failed: active_tx closed".into(),
                ));
            }
            Ok(())
        } else {
            tracing::warn!(
                agent = %self.card.id,
                "request_review 无处投递：active_tx=None；调用方应先 dispatch 再 nudge"
            );
            Err(CoreError::Other(
                "request_review failed: no active dispatch (active_tx=None)".into(),
            ))
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.status = AgentStatus::Stopping;
        inner.active_tx = None;
        // 主动 shutdown：拿走 death_tx，抑制 pump 退出时重复发信号——
        // Fuxi::shutdown 自己会显式发 AgentDead，这里不重。
        inner.death_tx = None;

        // WS channel 跟着 inner 一起 drop 会触发 server 停；这里显式清子进程。
        if let Some(mut child) = inner.child.take() {
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    // SAFETY: `kill(2)` 对已退出进程返回 ESRCH；best-effort。
                    unsafe {
                        libc_kill(pid as i32, SIGTERM);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
            }
            let wait = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            if wait.is_err() {
                tracing::warn!(agent = %self.card.id, "cc SIGTERM 超时 5s，升级到 SIGKILL");
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        inner.status = AgentStatus::Dead;
        tracing::info!(agent = %self.card.id, "cc agent shutdown complete");
        Ok(())
    }
}

/// 抽出 `request_review` 的事件构造逻辑，方便单测——adapter method 只剩
/// "lock + clone active_tx + send" 投递骨架，被 e2e 覆盖即可。
fn build_review_event(
    agent_id: AgentId,
    task_id: TaskId,
    topic_id: Option<fuxi_core::TopicId>,
    deliverable_kind: DeliverableKind,
    summary: String,
    artifact_ref: Option<String>,
) -> Event {
    let mut meta = EventMeta::now();
    meta.agent = Some(agent_id);
    meta.task = Some(task_id);
    meta.topic_id = topic_id;
    Event {
        meta,
        kind: EventKind::AgentRequestReview {
            agent: agent_id,
            task: task_id,
            deliverable_kind,
            summary,
            artifact_ref,
        },
    }
}

/// Bug A · 摊回 Idle + drain pending 的入口分类，让 [`drain_pending_to_idle`]
/// 共用三条路径——pump terminal / interrupt ack / 30s watchdog。
#[derive(Debug)]
enum DrainCause {
    /// 正常 turn 结束（cc emit ResultSuccess/Error）。
    TurnTerminal,
    /// cc 收 interrupt 后回的 control_response.subtype="success" ack。
    InterruptAck { request_id: String },
    /// cancel 后 30s 没等到 ack 也没等到 terminal——强制摊回 + 警告事件。
    WatchdogTimeout { expected_generation: u64 },
}

/// 长跑 pump：拉 WS 消息 → 路由。
///
/// 路由规则：
/// - `control_request/can_use_tool` → v0.1 全 `allow`（策略层 v0.2 再加）
/// - `control_request/interrupt` 的 **ack** 来自 claude 的 `control_response`，
///   subtype=success 时（Bug A 修）走 [`drain_pending_to_idle`] 同 turn-terminal
///   路径——cc 这之后不一定再 emit ResultSuccess/Error，等不到就是用户感受
///   到的"假死"。
/// - `keep_alive` / `rate_limit_event` → 日志或 Custom 事件（目前走 parse_line 兜底）
/// - 其它 → parse_line + translate → 发到当前 active_tx
async fn pump_loop(
    channel: Arc<WsChannel>,
    agent_id: AgentId,
    pid_hint: Option<u32>,
    inner: Arc<Mutex<Inner>>,
) {
    while let Some(msg) = channel.recv().await {
        // control_request：最高优先级，立即回复
        if msg.get("type").and_then(Value::as_str) == Some("control_request") {
            handle_control_request(&channel, &msg).await;
            continue;
        }
        // control_response：我方发 interrupt 的 ack。Bug A 修：subtype=success 时
        // 走 drain（让 status 摊回 Idle、pending 立即送出）。
        if msg.get("type").and_then(Value::as_str) == Some("control_response") {
            tracing::debug!(msg = ?msg, "control_response acked");
            let resp = msg.get("response");
            let subtype = resp
                .and_then(|r| r.get("subtype"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // request_id 可能在顶层（早期 anya wire）或 response 内（spec），都收。
            let req_id = msg
                .get("request_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    resp.and_then(|r| r.get("request_id"))
                        .and_then(Value::as_str)
                })
                .map(str::to_string);
            if subtype == "success"
                && let Some(rid) = req_id
            {
                let (task_id, topic_id) = {
                    let g = inner.lock().await;
                    (g.current_task, g.current_topic)
                };
                drain_pending_to_idle(
                    &inner,
                    agent_id,
                    task_id,
                    topic_id,
                    DrainCause::InterruptAck { request_id: rid },
                )
                .await;
            }
            continue;
        }

        // 其它（system / assistant / user tool_result / result / rate_limit / ...）
        // 序列化回 NDJSON line 喂给 parse_line——复用既有翻译规则表。
        let line = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "ws_pump: serialize-for-parse failed");
                continue;
            }
        };
        let cc_ev = match parse_line(&line) {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!(error = %e, line_preview = %line.chars().take(200).collect::<String>(), "ws_pump: parse failed");
                continue;
            }
        };

        let is_terminal = matches!(
            cc_ev,
            parser::CcEvent::ResultSuccess { .. } | parser::CcEvent::ResultError { .. }
        );

        // 拿锁产出 events，拿完锁立刻释放，避免 send 时占锁
        let (events_to_send, topic_hint) = {
            let mut guard = inner.lock().await;
            let task_id = guard.current_task;
            let topic_id = guard.current_topic;
            let mut new_state = std::mem::take(&mut guard.translate_state);
            let events = translate(cc_ev, agent_id, task_id, &mut new_state, pid_hint);
            if is_terminal && new_state.finish() {
                // 推一条 ThinkingFinished 兜底
                let mut meta = fuxi_core::event::EventMeta::now();
                meta.agent = Some(agent_id);
                meta.task = task_id;
                meta.topic_id = topic_id;
                let extra = Event {
                    meta,
                    kind: fuxi_core::event::EventKind::ThinkingFinished,
                };
                let mut combined = events;
                combined.push(extra);
                guard.translate_state = new_state;
                (combined, topic_id)
            } else {
                guard.translate_state = new_state;
                (events, topic_id)
            }
        };

        // Phase 1 #6：translate() / mk_event 不感知 topic_id（避免改 parser 大面积
        // signature）；这里统一在 emit 前 patch `meta.topic_id`——只补 None 不覆盖
        // 已有值。SystemEventBridge 凭此过滤跨 topic 的 worker 事件。
        let mut events_to_send = events_to_send;
        if topic_hint.is_some() {
            for ev in events_to_send.iter_mut() {
                if ev.meta.topic_id.is_none() {
                    ev.meta.topic_id = topic_hint;
                }
            }
        }

        // 发到 active_tx——clone sender 再释锁
        let tx_opt = {
            let guard = inner.lock().await;
            guard.active_tx.clone()
        };
        if let Some(tx) = tx_opt {
            for ev in events_to_send {
                if tx.send(ev).await.is_err() {
                    tracing::debug!(
                        "ws_pump: active_tx closed; future events will be dropped until next dispatch"
                    );
                    // 清掉 active_tx——但只清那一个
                    let mut guard = inner.lock().await;
                    guard.active_tx = None;
                    break;
                }
            }
        } else {
            tracing::trace!("ws_pump: no active_tx; event dropped (no dispatch in flight)");
        }

        if is_terminal {
            // terminal 后，不再 auto-drop active_tx——调用方可能要留着用 send_message 追加
            // 任务完成的认定由 orchestrator 层看到 TaskStateChanged → Done 来决。
            //
            // M2.1 · 消息黑洞修：turn 结束此刻是 cc 重新 poll WS 的窗口——把 pending
            // 队列的 follow-up 消息按 FIFO drain 自送。Bug A 后该逻辑被提到
            // [`drain_pending_to_idle`] 让 interrupt-ack / watchdog 共用同一路径。
            let (task_id, topic_id) = {
                let g = inner.lock().await;
                (g.current_task, g.current_topic)
            };
            drain_pending_to_idle(
                &inner,
                agent_id,
                task_id,
                topic_id,
                DrainCause::TurnTerminal,
            )
            .await;
        }
    }
    tracing::info!("ws_pump: channel closed, pump exiting");
    // 死亡信号：pump 退出意味着 cc 侧 WS 断了——正常 shutdown 会先把 death_tx 拿走，
    // 所以此时 death_tx 还在就代表意外死亡。发一条，Fuxi 侧转发 AgentDead。
    // why take：确保单发，避免重复 publish。
    let death_tx = {
        let mut guard = inner.lock().await;
        guard.death_tx.take()
    };
    if let Some(tx) = death_tx {
        let _ = tx.send("ws channel closed".to_string());
    }
}

/// Bug A · 把 status 摊回 Idle + drain pending → 锁外送回 channel。
///
/// 三条入口共享：
/// - `TurnTerminal`：cc emit ResultSuccess/Error（最常态）
/// - `InterruptAck`：cc 收到 interrupt 后回的 control_response.subtype="success"
/// - `WatchdogTimeout`：cancel 后 30s 兜底（cc 死活不 ack 也不 emit terminal）
///
/// 排他性靠 `cancel_generation` + `last_interrupt_request_id` + status 三重检查
/// 保证：
/// 1. 旧 watchdog 醒来发现 generation 已被新 cancel bump → silent return（避免
///    一次 cancel 触发两次 watchdog）
/// 2. interrupt-ack 命中 request_id 但 status 已 Idle（被 watchdog 抢先摊回） →
///    silent return，不重复 drain
/// 3. interrupt-ack 收到了**别人的** request_id（不 match 自己最近一次 cancel
///    的）→ silent return（防御 race）
async fn drain_pending_to_idle(
    inner: &Arc<Mutex<Inner>>,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    topic_id: Option<fuxi_core::TopicId>,
    cause: DrainCause,
) {
    let (channel_clone, cli_sid, drained, emit_warning) = {
        let mut guard = inner.lock().await;
        match &cause {
            DrainCause::TurnTerminal => {
                // 不做条件判定——turn 真到头就摊回（保留 M2.1 行为）
            }
            DrainCause::InterruptAck { request_id } => {
                // 不命中我们最近一次 cancel 的 request_id → 不是我们关心的 ack
                if guard.last_interrupt_request_id.as_deref() != Some(request_id.as_str()) {
                    return;
                }
                // 已被 watchdog / terminal 抢先摊回 → 不重复 drain
                if !matches!(guard.status, AgentStatus::Stopping) {
                    return;
                }
            }
            DrainCause::WatchdogTimeout {
                expected_generation,
            } => {
                // 新 cancel 已 bump generation → 旧 watchdog 失效
                if guard.cancel_generation != *expected_generation {
                    return;
                }
                // 已被 ack drain / terminal 摊回 → silent return
                if !matches!(guard.status, AgentStatus::Stopping) {
                    return;
                }
            }
        }
        guard.status = AgentStatus::Idle;
        // 清掉 last_interrupt_request_id：防止下一轮 cc emit 同 id 的 stale ack
        // 误命中本次 drain 已处理过的状态。
        guard.last_interrupt_request_id = None;
        let drained = guard.pending.drain_all();
        let cli_sid = guard.channel.cli_session_id().await.unwrap_or_default();
        let warn = matches!(cause, DrainCause::WatchdogTimeout { .. });
        (guard.channel.clone(), cli_sid, drained, warn)
    };
    for body in drained {
        let msg = CcAgent::user_message_value(&body, &cli_sid);
        if let Err(e) = channel_clone.send(msg) {
            tracing::warn!(error = %e, "drain：channel send 失败（channel 已关？）");
            break;
        }
    }
    // watchdog 兜底必发 AgentInterrupted{stopping_timeout}——让玄女自检
    // ("我打了 interrupt 但 cc 30s 没 ack，可能要重启")。active_tx=None 时仅日志。
    if emit_warning {
        tracing::warn!(
            agent = %agent_id,
            "stopping watchdog 30s 未见 ack/terminal，强制摊回 Idle 并发出 AgentInterrupted{{stopping_timeout}}"
        );
        let tx_opt = inner.lock().await.active_tx.clone();
        if let Some(tx) = tx_opt {
            let mut meta = EventMeta::now();
            meta.agent = Some(agent_id);
            meta.task = task_id;
            meta.topic_id = topic_id;
            let ev = Event {
                meta,
                kind: EventKind::AgentInterrupted {
                    reason: "stopping_timeout".to_string(),
                },
            };
            if tx.send(ev).await.is_err() {
                tracing::debug!(
                    agent = %agent_id,
                    "stopping_timeout 事件投递失败：active_tx 已关闭"
                );
            }
        } else {
            tracing::warn!(
                agent = %agent_id,
                "stopping_timeout 无处投递：active_tx=None"
            );
        }
    }
}

/// 处理 CLI 发来的 control_request。
async fn handle_control_request(channel: &WsChannel, msg: &Value) {
    let subtype = msg
        .get("request")
        .and_then(|r| r.get("subtype"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match subtype {
        "can_use_tool" => {
            // v0.1 全 allow（策略层 v0.2）
            let request_id = msg
                .get("request_id")
                .cloned()
                .unwrap_or(Value::String(uuid::Uuid::new_v4().to_string()));
            let input = msg
                .get("request")
                .and_then(|r| r.get("input"))
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let reply = json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {
                        "behavior": "allow",
                        "updatedInput": input,
                    }
                }
            });
            if let Err(e) = channel.send(reply) {
                tracing::warn!(error = %e, "can_use_tool allow reply send failed");
            }
        }
        other => {
            tracing::debug!(subtype = other, "unhandled control_request subtype");
        }
    }
}

// ── Unix signal helper ──
#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
#[cfg(unix)]
#[inline]
unsafe fn libc_kill(pid: i32, sig: i32) {
    // SAFETY: FFI to libc's `kill(2)`; arguments are plain integers.
    unsafe {
        kill(pid, sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_value_is_wellformed() {
        let v = CcAgent::user_message_value("hello", "sess-1");
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"][0]["type"], "text");
        assert_eq!(v["message"]["content"][0]["text"], "hello");
        assert_eq!(v["session_id"], "sess-1");
    }

    #[test]
    fn cc_error_into_core_preserves_variant() {
        let e: CoreError = CcError::Other("boom".into()).into();
        assert!(matches!(e, CoreError::Other(_)));
    }

    #[test]
    fn early_exit_error_maps_to_core_other() {
        let e: CoreError = CcError::EarlyExit.into();
        assert!(matches!(e, CoreError::Other(_)));
    }

    /// 编译断言：CcAgent 必须 override `Agent::session_id`（P2 召回依赖此）。
    /// 真起 cc 太重不在单测；此测保证签名/默认 None 退化不会回潮。
    /// 取 `Agent::session_id` 函数指针隐式要求 trait method 存在；CcAgent 实现 Agent
    /// 即可通过——若哪天误删 override 退化为默认 None，dispatch_pump 召回链断裂会被
    /// orchestrator 的集成测试抓到，单测层这里只守 trait 契约。
    #[test]
    fn cc_agent_implements_session_id_via_agent_trait() {
        fn assert_agent<A: Agent>() {}
        assert_agent::<CcAgent>();
    }

    /// `request_review` 事件构造：agent / task 必须填到 `meta` + `kind` 双处，
    /// kind 字段必须如实回显——下游 EventStore 持久化和 bridge attention filter
    /// 都按这两套字段读。
    #[test]
    fn build_review_event_populates_meta_and_kind_fields() {
        let agent = AgentId::new();
        let task = TaskId::new();
        let ev = build_review_event(
            agent,
            task,
            None,
            DeliverableKind::CodeChange,
            "三绿等审".into(),
            Some("sha:abc".into()),
        );
        assert_eq!(ev.meta.agent, Some(agent));
        assert_eq!(ev.meta.task, Some(task));
        assert!(
            ev.meta.topic_id.is_none(),
            "topic_id None 当无显式 topic 时"
        );
        match ev.kind {
            EventKind::AgentRequestReview {
                agent: kagent,
                task: ktask,
                deliverable_kind,
                summary,
                artifact_ref,
            } => {
                assert_eq!(kagent, agent);
                assert_eq!(ktask, task);
                assert_eq!(deliverable_kind, DeliverableKind::CodeChange);
                assert_eq!(summary, "三绿等审");
                assert_eq!(artifact_ref.as_deref(), Some("sha:abc"));
            }
            other => panic!("expected AgentRequestReview, got {other:?}"),
        }
    }

    /// `artifact_ref=None` 在 build 阶段就该被原样保留——summary-only deliverable
    /// （research_summary / decision_request）的常见形态。
    #[test]
    fn build_review_event_preserves_artifact_ref_none() {
        let ev = build_review_event(
            AgentId::new(),
            TaskId::new(),
            None,
            DeliverableKind::ResearchSummary,
            "看完 auth".into(),
            None,
        );
        match ev.kind {
            EventKind::AgentRequestReview { artifact_ref, .. } => {
                assert!(artifact_ref.is_none());
            }
            other => panic!("expected AgentRequestReview, got {other:?}"),
        }
    }
}
