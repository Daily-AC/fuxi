//! `CodexAgent`——实现 `fuxi_core::Agent` 的 Codex CLI 门客。
//!
//! 与 `CcAgent` 的关键差异：
//! - **懒 spawn**：因为 codex exec 的 prompt 必须在进程 argv 里，我们不能在
//!   `launch` 时启动进程——那时还没有 Task。`launch` 只保存 config/profile，
//!   `dispatch` 时才真正起进程。
//! - **一次 dispatch = 一次进程**：codex exec 跑完一轮就退出（无 stdin 追写），
//!   再次 dispatch 会启新子进程。对应地，`send_message` 在本适配器里**不支持**
//!   ——直接返回错误，让上层知道要换路径。
//! - **单次派发语义**：目前 `CodexAgent` 只跟踪「最近一次 dispatch 产生的
//!   child」；上游暂时没有并发多 task 的需求。

use crate::config::CodexLaunchConfig;
use crate::parser::{self, TranslateState, parse_line, translate};
use crate::spawn::{SpawnedCodex, spawn_codex};
use async_trait::async_trait;
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::Event;
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::Task;
use fuxi_core::{CoreError, Result};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{Mutex, mpsc};

/// Agent 事件 channel 的默认缓冲。
const EVENT_CHANNEL_BUFFER: usize = 32;

/// `fuxi-agent-codex` 的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum CodexError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("core: {0}")]
    Core(#[from] CoreError),
    #[error("codex process already exited")]
    Exited,
    #[error("{0}")]
    Other(String),
}

impl From<CodexError> for CoreError {
    fn from(err: CodexError) -> Self {
        match err {
            CodexError::Io(e) => CoreError::Io(e),
            CodexError::Serde(e) => CoreError::Serde(e),
            CodexError::Core(e) => e,
            other => CoreError::Other(other.to_string()),
        }
    }
}

/// 可变内部状态——最近一次 dispatch 起的 child + 当前 status。
struct Inner {
    /// 最近一次 dispatch 起的 child；None 表示尚未派发或已 wait。
    child: Option<Child>,
    /// 最近一次 child 的 pid，供 `AgentReady.endpoint` 展示。
    last_pid: Option<u32>,
    status: AgentStatus,
}

/// Codex CLI 门客。
///
/// 为什么持有 `cfg` 副本而不是 spawn 句柄：codex exec 是 one-shot，每次
/// dispatch 都会 fork 新进程；config 在多个 dispatch 之间复用。
pub struct CodexAgent {
    card: AgentCard,
    cfg: CodexLaunchConfig,
    inner: Arc<Mutex<Inner>>,
}

impl CodexAgent {
    /// 构造一个 `CodexAgent`——**不**立即起进程。
    ///
    /// 为什么 `launch` 不 spawn：codex exec 的 prompt 是位置参数，必须在 Task
    /// 到来时才有。懒 spawn 避免「起了进程但还没 prompt，codex 开始读 stdin
    /// 假死」的坑。
    ///
    /// 内部只是给 `launch_with_id` 套个新 `AgentId::new()` 的便利层。新业务代码
    /// 都应该走 `launch_with_id`——编排层（玄女）必须做唯一 id 真相源（S1 教训）。
    pub async fn launch(profile: AgentProfile, cfg: CodexLaunchConfig) -> Result<Self> {
        Self::launch_with_id(AgentId::new(), profile, cfg).await
    }

    /// 编排层指定 id 版本——解决 S1 教训的 AgentId 双生问题。
    ///
    /// 与 `CcAgent::launch_with_id` 签名对齐（`async fn`），方便
    /// `Fuxi::spawn_worker` 用同一个 await 语法在不同 `WorkerKind` 分支间路由。
    /// 实际 codex 不需要任何异步初始化（懒 spawn），但保持 trait 签名一致比省一个
    /// `await` 更值得。
    pub async fn launch_with_id(
        id: AgentId,
        profile: AgentProfile,
        cfg: CodexLaunchConfig,
    ) -> Result<Self> {
        let card = AgentCard {
            id,
            profile,
            // 没 spawn 过所以没有 pid；dispatch 时会变。
            endpoint: "pid:unspawned".to_string(),
            status: AgentStatus::Idle,
        };
        let inner = Arc::new(Mutex::new(Inner {
            child: None,
            last_pid: None,
            status: AgentStatus::Idle,
        }));
        Ok(Self { card, cfg, inner })
    }

    /// Task → codex 的 prompt 字符串。
    fn task_to_prompt(task: &Task) -> String {
        if task.description.is_empty() {
            task.title.clone()
        } else {
            format!("{}\n\n{}", task.title, task.description)
        }
    }
}

#[async_trait]
impl Agent for CodexAgent {
    fn card(&self) -> &AgentCard {
        &self.card
    }

    async fn dispatch(&self, task: Task) -> Result<mpsc::Receiver<Event>> {
        let prompt = Self::task_to_prompt(&task);
        let SpawnedCodex { child, stdout, pid } =
            spawn_codex(&self.cfg, &prompt).map_err(CodexError::Io)?;

        {
            let mut inner = self.inner.lock().await;
            // 若之前还有 child，先 drop 掉——kill_on_drop 会兜底清理。
            inner.child = Some(child);
            inner.last_pid = pid;
            inner.status = AgentStatus::Busy;
        }

        let (tx, rx) = mpsc::channel::<Event>(EVENT_CHANNEL_BUFFER);
        let agent_id = self.card.id;
        let task_id = Some(task.id);
        let inner_weak = Arc::downgrade(&self.inner);

        tokio::spawn(async move {
            reader_loop(stdout, tx, agent_id, task_id, pid).await;
            if let Some(inner) = inner_weak.upgrade() {
                let mut guard = inner.lock().await;
                guard.status = AgentStatus::Idle;
            }
        });

        Ok(rx)
    }

    /// codex exec 是 one-shot——无法在进程运行中注入第二条消息。
    /// 编排层要追加内容请起新 task（即一次新的 dispatch）。
    async fn send_message(&self, _task_id: TaskId, _text: &str) -> Result<()> {
        Err(CoreError::Other(
            "codex exec mode does not support follow-up messages; issue a new dispatch instead"
                .into(),
        ))
    }

    async fn cancel(&self, _task_id: TaskId) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    // SAFETY: `kill(2)` 接收 raw pid + signo；对已退出进程返回 ESRCH。
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
        tracing::info!(agent = %self.card.id, "sent cancel (SIGINT) to codex");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.status = AgentStatus::Stopping;
        if let Some(mut child) = inner.child.take() {
            let wait = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
            if wait.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        inner.status = AgentStatus::Dead;
        tracing::info!(agent = %self.card.id, "codex agent shutdown complete");
        Ok(())
    }
}

/// stdout 按行读 → parse → translate → tx。
/// 终止条件：`turn.completed` / `turn.failed` / EOF。
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
                tracing::error!(error = %e, "codex stdout read failed");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let cx_ev = match parse_line(&line) {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!(error = %e, line = %line, "codex line parse failed");
                continue;
            }
        };
        let is_terminal = matches!(
            cx_ev,
            parser::CodexEvent::TurnCompleted { .. } | parser::CodexEvent::TurnFailed { .. }
        );
        let events = translate(cx_ev, agent_id, task_id, &mut state, pid_hint);
        for ev in events {
            if tx.send(ev).await.is_err() {
                tracing::debug!("codex agent event channel closed by subscriber");
                return;
            }
        }
        if is_terminal {
            // codex 会紧接着 close stdout；我们不继续读，避免拖尾。
            return;
        }
    }
}

// ── Unix signal helper ──────────────────────────────────────────
// 复用 libc `kill(2)` FFI，避免为了一个信号引入整个 libc crate。
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

    fn profile() -> AgentProfile {
        AgentProfile {
            name: "codex-test".into(),
            role: "worker".into(),
            cli: "codex".into(),
            system_prompt: String::new(),
            tags: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn task_to_prompt_with_description_joins() {
        let mut t = Task::new("title", "desc");
        assert_eq!(CodexAgent::task_to_prompt(&t), "title\n\ndesc");
        t.description = String::new();
        assert_eq!(CodexAgent::task_to_prompt(&t), "title");
    }

    #[test]
    fn codex_error_into_core_preserves_other_variant() {
        let e: CoreError = CodexError::Other("boom".into()).into();
        assert!(matches!(e, CoreError::Other(_)));
    }

    #[tokio::test]
    async fn send_message_returns_error_in_exec_mode() {
        let agent = CodexAgent::launch(profile(), CodexLaunchConfig::default())
            .await
            .expect("launch");
        let res = agent.send_message(TaskId::new(), "hi").await;
        let err = res.expect_err("send_message should reject in exec mode");
        // 错误文本里必须提到 follow-up/exec，方便上层日志排障。
        let msg = format!("{err}");
        assert!(
            msg.contains("follow-up") || msg.contains("exec"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn launch_does_not_spawn_any_child() {
        let agent = CodexAgent::launch(profile(), CodexLaunchConfig::default())
            .await
            .expect("launch");
        let inner = agent.inner.lock().await;
        assert!(inner.child.is_none());
        assert_eq!(inner.status, AgentStatus::Idle);
    }

    /// S1 教训守门：`launch_with_id` 必须保留 caller 给的 id，不能内部 `AgentId::new()`。
    /// 没这条 lifecycle 事件 (AgentSpawning/AgentReady) 会属于不同 id，shelf 永远 Busy。
    #[tokio::test]
    async fn launch_with_id_preserves_caller_id() {
        let want = AgentId::new();
        let agent = CodexAgent::launch_with_id(want, profile(), CodexLaunchConfig::default())
            .await
            .expect("launch_with_id");
        assert_eq!(agent.card().id, want);
    }

    /// 编译时 trait 实装断言——若 `impl Agent for CodexAgent` 被误删，本测试不通过。
    #[test]
    fn codex_agent_implements_agent_trait() {
        fn assert_agent<T: Agent>() {}
        assert_agent::<CodexAgent>();
    }
}
