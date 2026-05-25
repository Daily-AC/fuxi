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

use crate::config::{CodexLaunchConfig, compose_prompt};
use crate::parser::{self, TranslateState, parse_line, translate};
use crate::spawn::{SpawnedCodex, spawn_codex};
use async_trait::async_trait;
use fuxi_core::agent::{Agent, AgentCard, AgentProfile, AgentStatus};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_core::task::Task;
use fuxi_core::{CoreError, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    ///
    /// `profile.system_prompt` 非空时会 prepend 进 prompt——这是 codex 获得 role
    /// 心智的唯一途径（codex exec 不吃 `--append-system-prompt`，见 `compose_prompt`
    /// 注释）。
    fn task_to_prompt(&self, task: &Task) -> String {
        compose_prompt(
            &self.card.profile.system_prompt,
            &task.title,
            &task.description,
        )
    }
}

#[async_trait]
impl Agent for CodexAgent {
    fn card(&self) -> &AgentCard {
        &self.card
    }

    async fn dispatch(&self, task: Task) -> Result<mpsc::Receiver<Event>> {
        let prompt = self.task_to_prompt(&task);
        let SpawnedCodex {
            child,
            stdout,
            stderr,
            pid,
        } = spawn_codex(&self.cfg, &prompt).map_err(CodexError::Io)?;

        // v2-session13：dispatch 时只持 stderr handle 给 collector，child
        // 由 spawn task 独占（reader_loop 退出后 wait child 拿 exit code）。
        // 之前 child 寄存在 inner——但 wait child 必须独占所有权，需要 take
        // 出来。Cancel 路径（user-driven SIGINT）现在依赖 last_pid + libc kill
        // 而非 inner.child，因此 child 不放回 inner 也不影响 cancel 语义。
        let pre_model = self.cfg.model.clone();
        {
            let mut inner = self.inner.lock().await;
            // 之前残留的 child 直接 drop——kill_on_drop 兜底清理。
            inner.child = None;
            inner.last_pid = pid;
            inner.status = AgentStatus::Busy;
        }

        let (tx, rx) = mpsc::channel::<Event>(EVENT_CHANNEL_BUFFER);
        let agent_id = self.card.id;
        let task_id = Some(task.id);
        let inner_weak = Arc::downgrade(&self.inner);

        // stderr collector：异步追读，缓冲到 Vec<String>。reader_loop 退出后
        // 上层从这取末尾几行作为诊断信息。
        let stderr_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf_for_loop = stderr_buf.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let mut g = stderr_buf_for_loop.lock().await;
                if g.len() >= 64 {
                    g.remove(0);
                }
                g.push(line);
            }
        });

        let emit_count = Arc::new(AtomicUsize::new(0));
        let emit_count_for_reader = emit_count.clone();
        let mut child = child;

        tokio::spawn(async move {
            reader_loop(
                stdout,
                tx.clone(),
                agent_id,
                task_id,
                pid,
                emit_count_for_reader,
            )
            .await;

            // reader_loop 已结束（终态/EOF/错误其一）→ 等 child 真死拿 exit code
            let exit_status = match child.wait().await {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(error = %e, agent = %agent_id, "codex wait child 失败");
                    None
                }
            };
            let exit_ok = exit_status.map(|s| s.success()).unwrap_or(false);
            let exit_code = exit_status.and_then(|s| s.code());
            let emitted = emit_count.load(Ordering::SeqCst);

            // 异常退出诊断：codex 进程没翻译出任何事件且非 0 退出。原因常见：
            // - 未设 FUXI_CODEX_MODEL（API key 账号 codex 拒绝默认模型）
            // - codex auth 失效（chatgpt/api key 过期）
            // - codex binary 不在 PATH（spawn 阶段已挂，本路径不会走到）
            // 翻译成 AgentResponded 让玄女在对话流里看到、能给用户复述。
            if emitted == 0 && !exit_ok {
                let stderr_tail = {
                    let g = stderr_buf.lock().await;
                    g.iter()
                        .rev()
                        .take(8)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                let hint = if pre_model.is_empty() {
                    "可能原因：未设 FUXI_CODEX_MODEL（API key 账号需要显式指定，\
                     如 `export FUXI_CODEX_MODEL=gpt-5.1-mini`），或 codex auth 失效。"
                } else {
                    "可能原因：FUXI_CODEX_MODEL 当前值不被账号支持，或 codex auth 失效。"
                };
                let exit_str = exit_code
                    .map(|c| format!("exit={c}"))
                    .unwrap_or_else(|| "exit=?".to_string());
                let mut msg = format!("codex 子进程异常退出（{exit_str}），未产出任何事件。{hint}");
                if !stderr_tail.is_empty() {
                    msg.push_str("\n\nstderr 末尾：\n");
                    msg.push_str(&stderr_tail);
                }
                tracing::warn!(
                    agent = %agent_id,
                    exit = ?exit_code,
                    "codex 静默失败兜底诊断"
                );
                let mut meta = EventMeta::now();
                meta.agent = Some(agent_id);
                meta.task = task_id;
                let _ = tx
                    .send(Event {
                        meta,
                        kind: EventKind::AgentResponded {
                            text: msg,
                            artifact_ref: None,
                        },
                    })
                    .await;
                let mut meta = EventMeta::now();
                meta.agent = Some(agent_id);
                meta.task = task_id;
                let cause = exit_code
                    .map(|c| format!("codex exit {c}"))
                    .unwrap_or_else(|| "codex exit unknown".to_string());
                let _ = tx
                    .send(Event {
                        meta,
                        kind: EventKind::AgentDead { cause },
                    })
                    .await;
            }

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
        // v2-session13：child 现由 dispatch spawn task 独占（reader_loop 退出后
        // wait 它），inner 不再持。改用 last_pid + libc kill 信号路径——对已退
        // 进程 ESRCH 是 noop，不会误伤。
        if let Some(pid) = inner.last_pid {
            #[cfg(unix)]
            {
                // SAFETY: `kill(2)` 接收 raw pid + signo；对已退出进程返回 ESRCH。
                unsafe {
                    libc_kill(pid as i32, SIGINT);
                }
            }
        }
        inner.status = AgentStatus::Stopping;
        tracing::info!(agent = %self.card.id, "sent cancel (SIGINT) to codex");
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.status = AgentStatus::Stopping;
        // child 已被 spawn task 拿走；shutdown 走 SIGINT 信号 + 信赖 spawn task
        // 的 kill_on_drop 兜底。无 wait（spawn task 自己会 wait）。
        if let Some(pid) = inner.last_pid {
            #[cfg(unix)]
            {
                unsafe {
                    libc_kill(pid as i32, SIGINT);
                }
            }
        }
        inner.status = AgentStatus::Dead;
        tracing::info!(agent = %self.card.id, "codex agent shutdown signaled");
        Ok(())
    }
}

/// stdout 按行读 → parse → translate → tx。
/// 终止条件：`turn.completed` / `turn.failed` / EOF。
///
/// `emit_count`：每条成功 send 的事件 +1。dispatch task 退出时若仍为 0 + child
/// exit != 0，emit 兜底诊断（避免 codex 异常退出"静默失败"）。
async fn reader_loop(
    stdout: ChildStdout,
    tx: mpsc::Sender<Event>,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    pid_hint: Option<u32>,
    emit_count: Arc<AtomicUsize>,
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
            emit_count.fetch_add(1, Ordering::SeqCst);
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

    #[tokio::test]
    async fn task_to_prompt_with_description_joins() {
        let agent = CodexAgent::launch(profile(), CodexLaunchConfig::default())
            .await
            .expect("launch");
        let mut t = Task::new("title", "desc");
        assert_eq!(agent.task_to_prompt(&t), "title\n\ndesc");
        t.description = String::new();
        assert_eq!(agent.task_to_prompt(&t), "title");
    }

    /// role 心智必须经由 `profile.system_prompt` 注入 prompt——否则 codex exec
    /// 完全不知道自己扮演什么 role（`--append-system-prompt` 是 cc 专属）。
    #[tokio::test]
    async fn task_to_prompt_prepends_profile_system_prompt() {
        let mut p = profile();
        p.system_prompt = "你是鲁班，擅长拆任务".into();
        let agent = CodexAgent::launch(p, CodexLaunchConfig::default())
            .await
            .expect("launch");
        let t = Task::new("第一步", "把任务拆成三段");
        let prompt = agent.task_to_prompt(&t);
        assert!(
            prompt.starts_with("你是鲁班，擅长拆任务\n\n---\n\n"),
            "got: {prompt}"
        );
        assert!(prompt.contains("第一步\n\n把任务拆成三段"), "got: {prompt}");
    }

    /// v2-session13: codex 异常退出（exit 非 0 + 0 事件）应兜底 emit 一条
    /// AgentResponded（含 FUXI_CODEX_MODEL 提示）+ 一条 AgentDead。
    /// 用 `/bin/sh -c "echo ... >&2; exit 1"` 模拟 codex auth 失败场景。
    #[cfg(unix)]
    #[tokio::test]
    async fn dispatch_emits_diagnostic_when_codex_exits_silently() {
        let cfg = CodexLaunchConfig {
            binary: "/bin/sh".into(),
            // sh -c "..." 会忽略后续 argv（codex 的 build_args + prompt），
            // 等于跑这条 script 然后 exit 1。stderr 一行让我们能验诊断里带 stderr。
            argv_prefix: vec![
                "-c".into(),
                "echo 'simulated codex auth failure: invalid_request_error' >&2; exit 1".into(),
                "_unused_argv0".into(),
            ],
            model: String::new(),
            full_auto: false,
            bypass_approvals: false,
            ..Default::default()
        };
        let agent = CodexAgent::launch_with_id(AgentId::new(), profile(), cfg)
            .await
            .expect("launch");
        let mut rx = agent.dispatch(Task::new("t", "d")).await.expect("dispatch");

        // 收齐所有 events（rx 关闭后跳出）
        let mut events = Vec::new();
        let collect = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(ev) = rx.recv().await {
                events.push(ev);
            }
        })
        .await;
        assert!(collect.is_ok(), "rx 应在 5s 内关闭（codex sh exit 1）");

        // 兜底诊断 AgentResponded：必含 FUXI_CODEX_MODEL 提示 + stderr 那行
        let diag = events.iter().find_map(|e| match &e.kind {
            fuxi_core::EventKind::AgentResponded { text, .. } => Some(text.clone()),
            _ => None,
        });
        let diag = diag.expect("应 emit 兜底 AgentResponded");
        assert!(
            diag.contains("FUXI_CODEX_MODEL"),
            "诊断应提示 FUXI_CODEX_MODEL，实得: {diag}"
        );
        assert!(
            diag.contains("simulated codex auth failure"),
            "诊断应含 stderr 末尾，实得: {diag}"
        );

        // 兜底 AgentDead
        let dead = events
            .iter()
            .any(|e| matches!(&e.kind, fuxi_core::EventKind::AgentDead { .. }));
        assert!(dead, "应 emit 兜底 AgentDead");
    }

    /// 反向回归：codex 正常退出（exit 0 + 至少 1 个事件）不应 emit 兜底。
    /// 用一段 jsonl mock：合法的 turn.completed 让 reader_loop emit 一条事件。
    #[cfg(unix)]
    #[tokio::test]
    async fn dispatch_does_not_emit_diagnostic_on_normal_exit() {
        // 用 sh -c 输出一行 codex jsonl 然后正常退出。
        // turn.completed 是终态——reader_loop 看到它会 return（不读 EOF）。
        let line = r#"{"type":"turn.completed","cost":0.0}"#;
        let cfg = CodexLaunchConfig {
            binary: "/bin/sh".into(),
            argv_prefix: vec![
                "-c".into(),
                format!("echo '{line}'; exit 0"),
                "_unused".into(),
            ],
            model: String::new(),
            full_auto: false,
            bypass_approvals: false,
            ..Default::default()
        };
        let agent = CodexAgent::launch_with_id(AgentId::new(), profile(), cfg)
            .await
            .expect("launch");
        let mut rx = agent.dispatch(Task::new("t", "d")).await.expect("dispatch");

        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(ev) = rx.recv().await {
                events.push(ev);
            }
        })
        .await;

        // 不应 emit 含 "FUXI_CODEX_MODEL" 的 AgentResponded
        let bogus_diag = events.iter().find_map(|e| match &e.kind {
            fuxi_core::EventKind::AgentResponded { text, .. }
                if text.contains("FUXI_CODEX_MODEL") =>
            {
                Some(text.clone())
            }
            _ => None,
        });
        assert!(
            bogus_diag.is_none(),
            "正常退出不该 emit 兜底诊断，实得: {bogus_diag:?}"
        );
        // 也不应 emit AgentDead（正常退出由 spawn task 默默清状态）
        let dead = events
            .iter()
            .any(|e| matches!(&e.kind, fuxi_core::EventKind::AgentDead { .. }));
        assert!(!dead, "正常退出不该 emit AgentDead");
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
