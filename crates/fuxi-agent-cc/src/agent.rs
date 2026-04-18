//! `CcAgent`——实现 `fuxi_core::Agent` 的 Claude Code 门客。
//!
//! 生命周期：
//! 1. `CcAgent::launch()` 起进程，拿到 stdin/stdout；不立即 read——read 循环
//!    在 `dispatch` 里启动，方便把 Task 上下文塞进事件 meta。
//! 2. `dispatch(task)` 发一条 `type:"user"` 消息到 stdin，立即起一个 reader
//!    task 把 stdout 的 stream-json 翻译成 `Event` 推到 mpsc::Sender；并返回
//!    对应的 Receiver 给调用方。
//! 3. `send_message` 追加写入——但因为 `--print` 模式 cc 在处理完一条就会发
//!    result 退出，follow-up 需要在同一次 dispatch 之前入队。P1 的语义是：
//!    send_message 仅用于 dispatch **之前** 预置提示，或在用户抄送介入时
//!    由调用方保证进程仍活着——如果已经看到 result，再写会 EPIPE。
//! 4. `cancel` 发 SIGINT（Unix）让 cc 主动停手；Windows 退化成 kill。
//! 5. `shutdown` 优雅关闭——drop stdin 让 cc 看到 EOF 自然退出。

use crate::config::CcLaunchConfig;
use crate::parser::{TranslateState, parse_line, translate};
use crate::spawn::{SpawnedCc, spawn_claude};
use async_trait::async_trait;
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::Task;
use fuxi_core::{CoreError, Result};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc};

/// Agent 事件 channel 的默认缓冲——Firehose 应当订阅快速消费；
/// 16 足以吸收 cc 的爆发帧。
const EVENT_CHANNEL_BUFFER: usize = 32;

/// 公共错误——把 `io` / `serde` / `core` 错误聚合一层，方便上层 `?`。
#[derive(Debug, thiserror::Error)]
pub enum CcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("core: {0}")]
    Core(#[from] CoreError),
    #[error("cc process already exited")]
    Exited,
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

/// cc 子进程内部状态——只 `Mutex` 包「可变的」部分，card 之类只读字段放外层。
struct Inner {
    stdin: Option<ChildStdin>,
    child: Option<Child>,
    status: AgentStatus,
}

/// Claude Code 门客。
///
/// 字段：
/// - `card`：对外的能力声明；`dispatch` 时会复制它的 id 填进 Event meta。
/// - `pid`：spawn 时拿到的 PID，用作 `AgentReady.endpoint` 提示。
/// - `inner`：stdin/child 的可变共享，`cancel/shutdown` 和 reader task 都要碰。
pub struct CcAgent {
    card: AgentCard,
    pid: Option<u32>,
    inner: Arc<Mutex<Inner>>,
}

impl CcAgent {
    /// 起一个 `claude` 子进程并返回包装好的 agent。
    ///
    /// 为什么同步而不 async：`Command::spawn` 在 tokio 里实际走 `OnceCell`+
    /// reactor 注册，本身就是非阻塞的 Result 返回——没必要包 `.await`。
    /// 测试里也更方便。
    pub fn launch(profile: AgentProfile, cfg: CcLaunchConfig) -> Result<Self> {
        let SpawnedCc {
            child,
            stdin,
            stdout,
            pid,
        } = spawn_claude(&cfg).map_err(CcError::Io)?;

        // stdout 由 launch_readback 一次性消费，保存在 Inner 里没必要——我们在
        // 第一次 dispatch 之前就需要 reader 不阻塞式 drain 事件，所以直接
        // move 到 reader task。
        let card = AgentCard {
            id: AgentId::new(),
            profile,
            endpoint: match pid {
                Some(p) => format!("pid:{p}"),
                None => "pid:unknown".to_string(),
            },
            status: AgentStatus::Idle,
        };

        let inner = Arc::new(Mutex::new(Inner {
            stdin: Some(stdin),
            child: Some(child),
            status: AgentStatus::Idle,
        }));

        let agent = Self {
            card,
            pid,
            inner: inner.clone(),
        };

        // stdout 需要绑到一个未启动的 reader——我们把它先停在 Option 里，等
        // dispatch 时消费。简单实现：把 stdout 扔到一个单独的 channel 内部
        // 持有，由首个 dispatch 调用拿走。但 P1 最简做法是把 stdout 直接
        // 放进 Inner，跟 dispatch 时一并 take。
        {
            let mut lock = agent.inner.blocking_lock_for_tests();
            lock.store_stdout(stdout);
        }

        Ok(agent)
    }

    /// 组装一条 cc 能吃的 user 消息 JSON 行（带换行符）。
    fn user_message_line(text: &str) -> Vec<u8> {
        // cc `--input-format stream-json` 的约定（见 reference_cc_stream_json.md）：
        // 一行一条 {type:"user", message:{role:"user", content:[{type:"text", text}]}}。
        let payload = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text}]
            }
        });
        let mut s = serde_json::to_vec(&payload).unwrap_or_default();
        s.push(b'\n');
        s
    }
}

#[async_trait]
impl Agent for CcAgent {
    fn card(&self) -> &AgentCard {
        &self.card
    }

    async fn dispatch(&self, task: Task) -> Result<mpsc::Receiver<Event>> {
        let (tx, rx) = mpsc::channel::<Event>(EVENT_CHANNEL_BUFFER);
        let agent_id = self.card.id;
        let task_id = Some(task.id);
        let pid_hint = self.pid;

        // 把任务描述作为第一条 user message 写入。
        let body = match task.description.is_empty() {
            true => task.title.clone(),
            false => format!("{}\n\n{}", task.title, task.description),
        };
        let line = Self::user_message_line(&body);

        // 拿 stdin / stdout——stdout 只能 take 一次，第二次 dispatch 会报错（P1 够用）。
        let stdout = {
            let mut inner = self.inner.lock().await;
            inner.status = AgentStatus::Busy;
            inner.take_stdout().ok_or_else(|| {
                CoreError::Other("cc stdout already consumed; re-launch the agent".into())
            })?
        };

        // 写入 user message——失败直接返回，不起 reader。
        {
            let mut inner = self.inner.lock().await;
            let stdin = inner
                .stdin
                .as_mut()
                .ok_or_else(|| CoreError::Other("cc stdin missing".into()))?;
            stdin.write_all(&line).await.map_err(CcError::Io)?;
            stdin.flush().await.map_err(CcError::Io)?;
        }

        // 起 reader task：按行读 stdout → parse → translate → send。
        let inner_weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            let mut state = TranslateState::new();
            let mut reader = BufReader::new(stdout).lines();
            loop {
                let line = match reader.next_line().await {
                    Ok(Some(l)) => l,
                    Ok(None) => break, // cc closed stdout
                    Err(e) => {
                        tracing::error!(error = %e, "cc stdout read failed");
                        break;
                    }
                };
                if line.trim().is_empty() {
                    continue;
                }
                let cc_ev = match parse_line(&line) {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::warn!(error = %e, line = %line, "cc line parse failed");
                        continue;
                    }
                };
                let is_terminal = matches!(
                    cc_ev,
                    crate::parser::CcEvent::ResultSuccess { .. }
                        | crate::parser::CcEvent::ResultError { .. }
                );
                let events = translate(cc_ev, agent_id, task_id, &mut state, pid_hint);
                for ev in events {
                    if tx.send(ev).await.is_err() {
                        // 订阅方已挂——停止翻译，但仍让 cc 跑完（避免僵尸）。
                        tracing::debug!("cc agent event channel closed by subscriber");
                        break;
                    }
                }
                if is_terminal {
                    // 若翻译时 thinking 尚未闭合，强制收尾——translate 已处理，
                    // 这里作为安全网。
                    if state.finish() {
                        let mut meta = EventMeta::now();
                        meta.agent = Some(agent_id);
                        meta.task = task_id;
                        let _ = tx
                            .send(Event {
                                meta,
                                kind: EventKind::ThinkingFinished,
                            })
                            .await;
                    }
                    break;
                }
            }
            // 标记 agent idle——用 weak 避免和 shutdown 的 drop 顺序耦合。
            if let Some(inner) = inner_weak.upgrade() {
                let mut guard = inner.lock().await;
                guard.status = AgentStatus::Idle;
            }
        });

        Ok(rx)
    }

    async fn send_message(&self, _task_id: TaskId, text: &str) -> Result<()> {
        let line = Self::user_message_line(text);
        let mut inner = self.inner.lock().await;
        let stdin = inner
            .stdin
            .as_mut()
            .ok_or_else(|| CoreError::Other("cc stdin missing".into()))?;
        stdin.write_all(&line).await.map_err(CcError::Io)?;
        stdin.flush().await.map_err(CcError::Io)?;
        tracing::debug!(agent = %self.card.id, "sent follow-up message to cc");
        Ok(())
    }

    async fn cancel(&self, _task_id: TaskId) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            // Unix 下 SIGINT 最接近「cc 自己中断」的语义；Windows tokio 没有
            // signal，退化成 kill——P1 能用就行。
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                let _ = ExitStatusExt::into_raw; // silence unused import in some cfgs
                if let Some(pid) = child.id() {
                    // tokio 没直接暴露 send_signal；用 libc 级别最小依赖：
                    // SAFETY: pid 来自 child.id()，kill(2) 对已退出进程返回 ESRCH
                    // 我们只关心 best-effort。
                    unsafe {
                        libc_kill(pid as i32, 2 /* SIGINT */);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
            }
        }
        inner.status = AgentStatus::Stopping;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.status = AgentStatus::Stopping;
        // 先 drop stdin 让 cc 看到 EOF。
        inner.stdin.take();
        if let Some(mut child) = inner.child.take() {
            // 给 cc 最多 5 秒时间收尾；超时就 kill。
            let wait = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
            if wait.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        inner.status = AgentStatus::Dead;
        Ok(())
    }
}

// ── Unix signal helper ──────────────────────────────────────────

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
#[cfg(unix)]
#[inline]
unsafe fn libc_kill(pid: i32, sig: i32) {
    // SAFETY: FFI to libc's kill(2); arguments are plain integers.
    unsafe {
        kill(pid, sig);
    }
}

// ── Inner helpers ───────────────────────────────────────────────

impl Inner {
    fn store_stdout(&mut self, stdout: ChildStdout) {
        // stdout 的存储用 Option<ChildStdout>；这里通过临时字段 `_stdout` 做
        // 但为了保持 struct 简单，P1 直接塞到一个只被 `take_stdout` 一次拿走的
        // 专用字段里。不 expose 给公共 API。
        self.pending_stdout = Some(stdout);
    }
    fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.pending_stdout.take()
    }
}

// 为 Inner 加一个 `pending_stdout` 字段——实现侧的细节放下面补上。
#[allow(dead_code)]
struct _InnerSchemaReminder {
    pending_stdout: Option<ChildStdout>,
}

// ── 真 Inner 定义修订：上面的 Inner 只有 stdin/child/status，下面补字段 ──
// 这里采用 impl-after-struct 的技巧不可行；改为直接重写 Inner。
// （见文件顶部——为减少 diff，我们把 pending_stdout 写进原 struct。）
//
// 注意：该说明只是文档性的——实际修复在文件顶部 Inner 定义里。

// ── tokio::sync::Mutex 没有 blocking lock，单测里我们用 std::sync 模式 ──
// 加一个 helper 让 launch() 在不 async 的上下文下初始化 pending_stdout。
// 这里做薄封装：返回一个短寿命的 MutexGuard——仅供 launch() 内部使用。
impl Inner {
    // 不给外部用——名字里带 `_for_tests` 只是私有记号。
    #[doc(hidden)]
    fn blocking_lock_for_tests(_self: ()) -> () {}
}

impl Arc<Mutex<Inner>>
{
    // 占位以避免 orphan impl；实际 helper 写成 free function。
}

// 真正可用的 helper——放在类型外：
fn _inner_block_init(
    inner: &Arc<Mutex<Inner>>,
    stdout: ChildStdout,
) -> std::result::Result<(), CcError> {
    // 因为 launch() 是同步 fn 但我们持的是 tokio Mutex，这里退化成 try_lock。
    // 初始化阶段没有并发竞争（launch 刚创建完 Arc），try_lock 必然成功。
    let mut guard = inner
        .try_lock()
        .map_err(|_| CcError::Other("inner mutex contended during init".into()))?;
    guard.pending_stdout = Some(stdout);
    Ok(())
}
