//! `CcAgent`——实现 `fuxi_core::Agent` 的 Claude Code 门客。
//!
//! 生命周期：
//! 1. `CcAgent::launch()` 起进程，拿到 stdin/stdout；不立即 read——read 循环
//!    在 `dispatch` 里启动，方便把 Task 上下文塞进事件 meta。
//! 2. `dispatch(task)` 发一条 `type:"user"` 消息到 stdin，立即起一个 reader
//!    task 把 stdout 的 stream-json 翻译成 `Event` 推到 mpsc::Sender；并返回
//!    对应的 Receiver 给调用方。读到 `result` 事件即关闭 channel。
//! 3. `send_message` 追加写入——但因为 `--print` 模式 cc 在处理完一条就发
//!    result 退出，follow-up 只在 dispatch **之前** 有意义；对已经看到 result
//!    的进程写会 EPIPE。P1 不做持续会话，返回错误即可。
//! 4. `cancel` 发 SIGINT（Unix）让 cc 主动停手；非 Unix 退化成 kill。
//! 5. `shutdown` 优雅关闭——drop stdin 让 cc 看到 EOF 自然退出，超时则 kill。

use crate::config::CcLaunchConfig;
use crate::parser::{self, TranslateState, parse_line, translate};
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

/// Agent 事件 channel 的默认缓冲——Firehose 应当快速消费；
/// 32 足以吸收 cc 的爆发帧。
const EVENT_CHANNEL_BUFFER: usize = 32;

/// `fuxi-agent-cc` 的错误类型——聚合 io / serde / core。
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

/// cc 子进程的可变内部状态——reader task / cancel / shutdown 都要碰。
struct Inner {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    child: Option<Child>,
    status: AgentStatus,
}

/// Claude Code 门客。
///
/// - `card`：对外能力声明；`dispatch` 时 id 会填进每条 Event meta。
/// - `pid`：spawn 拿到的 PID，用作 `AgentReady.endpoint` 提示。
/// - `inner`：stdin/stdout/child 的可变共享。
pub struct CcAgent {
    card: AgentCard,
    pid: Option<u32>,
    inner: Arc<Mutex<Inner>>,
}

impl CcAgent {
    /// 起一个 `claude` 子进程并返回包装好的 agent。
    ///
    /// 为什么 `launch` 同步：`tokio::process::Command::spawn` 本身非阻塞，
    /// 返回 `io::Result<Child>`；不 await 也不会阻塞 reactor。
    pub fn launch(profile: AgentProfile, cfg: CcLaunchConfig) -> Result<Self> {
        let SpawnedCc {
            child,
            stdin,
            stdout,
            pid,
        } = spawn_claude(&cfg).map_err(CcError::Io)?;

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
            stdout: Some(stdout),
            child: Some(child),
            status: AgentStatus::Idle,
        }));

        Ok(Self { card, pid, inner })
    }

    /// 组装一条 cc 能吃的 user 消息 JSON 行（带换行）。
    ///
    /// wire 格式：`{type:"user", message:{role:"user", content:[{type:"text", text}]}}\n`。
    fn user_message_line(text: &str) -> std::result::Result<Vec<u8>, serde_json::Error> {
        let payload = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": text}]
            }
        });
        let mut s = serde_json::to_vec(&payload)?;
        s.push(b'\n');
        Ok(s)
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

        let body = if task.description.is_empty() {
            task.title.clone()
        } else {
            format!("{}\n\n{}", task.title, task.description)
        };
        let line = Self::user_message_line(&body).map_err(CcError::Serde)?;

        // 先 take stdout——只有首个 dispatch 能起 reader（P1 够用：一次对话一次 cc）。
        let stdout = {
            let mut inner = self.inner.lock().await;
            inner.status = AgentStatus::Busy;
            inner.stdout.take().ok_or_else(|| {
                CoreError::Other(
                    "cc stdout already consumed; launch a new CcAgent per dispatch".into(),
                )
            })?
        };

        // 写 user message。
        {
            let mut inner = self.inner.lock().await;
            let stdin = inner
                .stdin
                .as_mut()
                .ok_or_else(|| CoreError::Other("cc stdin missing".into()))?;
            stdin.write_all(&line).await.map_err(CcError::Io)?;
            stdin.flush().await.map_err(CcError::Io)?;
        }

        let inner_weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            reader_loop(stdout, tx, agent_id, task_id, pid_hint).await;
            if let Some(inner) = inner_weak.upgrade() {
                let mut guard = inner.lock().await;
                guard.status = AgentStatus::Idle;
            }
        });

        Ok(rx)
    }

    async fn send_message(&self, _task_id: TaskId, text: &str) -> Result<()> {
        let line = Self::user_message_line(text).map_err(CcError::Serde)?;
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
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    // SAFETY: `kill(2)` 接收原始 pid + 信号值，对已退出进程返回 ESRCH；
                    // 我们只要 best-effort 通知。
                    unsafe {
                        libc_kill(pid as i32, SIGINT);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
            }
        }
        inner.status = AgentStatus::Stopping;
        tracing::info!(agent = %self.card.id, "sent cancel to cc");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.status = AgentStatus::Stopping;
        // drop stdin → cc 看到 EOF 自退。
        inner.stdin.take();
        if let Some(mut child) = inner.child.take() {
            let wait = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
            if wait.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        inner.status = AgentStatus::Dead;
        tracing::info!(agent = %self.card.id, "cc agent shutdown complete");
        Ok(())
    }
}

/// stdout 按行读 → parse → translate → tx。result 事件到达即返回。
async fn reader_loop(
    stdout: ChildStdout,
    tx: mpsc::Sender<Event>,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    pid_hint: Option<u32>,
) {
    let mut state = TranslateState::new();
    let mut reader = BufReader::new(stdout).lines();
    loop {
        let line = match reader.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
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
            parser::CcEvent::ResultSuccess { .. } | parser::CcEvent::ResultError { .. }
        );
        let events = translate(cc_ev, agent_id, task_id, &mut state, pid_hint);
        for ev in events {
            if tx.send(ev).await.is_err() {
                tracing::debug!("cc agent event channel closed by subscriber");
                return;
            }
        }
        if is_terminal {
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
            return;
        }
    }
}

// ── Unix signal helper ──────────────────────────────────────────
// 用 libc 的 `kill(2)` 而不是引入整个 libc crate——只需要一个 FFI 声明。
#[cfg(unix)]
const SIGINT: i32 = 2;
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
    fn user_message_line_is_wellformed() {
        let bytes = CcAgent::user_message_line("hello").expect("serialize");
        let s = std::str::from_utf8(&bytes).expect("utf8");
        assert!(s.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(s.trim_end()).expect("parse back");
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"][0]["type"], "text");
        assert_eq!(v["message"]["content"][0]["text"], "hello");
    }

    #[test]
    fn cc_error_into_core_preserves_variant() {
        let e: CoreError = CcError::Other("boom".into()).into();
        assert!(matches!(e, CoreError::Other(_)));
    }

    #[tokio::test]
    async fn reader_loop_handles_init_then_result() {
        // 想用内存 pipe 喂 reader_loop 但 DuplexStream 不等同于 ChildStdout——
        // 改走 parse_line + translate 的手工驱动，验证终局语义。
        let mut state = TranslateState::new();
        let agent = AgentId::new();
        let init = parse_line(
            r#"{"type":"system","subtype":"init","session_id":"s","model":"haiku","cwd":"/tmp"}"#,
        )
        .expect("parse");
        let res =
            parse_line(r#"{"type":"result","subtype":"success","result":"hi"}"#).expect("parse");

        let mut all = Vec::new();
        all.extend(translate(init, agent, None, &mut state, Some(7)));
        all.extend(translate(res, agent, None, &mut state, Some(7)));

        assert!(matches!(all[0].kind, EventKind::AgentReady { .. }));
        let last = all.last().expect("events present");
        assert!(matches!(last.kind, EventKind::AgentResponded { .. }));
    }
}
