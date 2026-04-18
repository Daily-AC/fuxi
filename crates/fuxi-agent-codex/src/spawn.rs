//! 起一个真实的 `codex exec` 子进程，返回 stdio 句柄。
//!
//! 为什么不做 stdin pipe：codex exec 的 prompt 是位置参数，一次性提供；
//! 进程对 stdin 只读一次（"Reading additional input from stdin..."），不支持
//! 多轮消息注入。我们**不保留** stdin pipe，避免误导调用方以为能追写。
//!
//! 为什么 spawn 从 Agent 层拆出：
//! 1. argv/env/cwd 细节独立可测；
//! 2. 错误边界更清晰（binary 不存在 vs stdout pipe 拿不到）。

use crate::config::CodexLaunchConfig;
use std::process::Stdio;
use tokio::process::{Child, ChildStdout, Command};

/// `spawn_codex` 的返回值。child 正在后台跑，stdout 已 pipe。
/// 不暴露 stdin——codex exec 不接受追加消息。
pub struct SpawnedCodex {
    pub child: Child,
    pub stdout: ChildStdout,
    pub pid: Option<u32>,
}

/// 启动 `codex exec` 子进程。
///
/// `prompt` 作为最后一个位置参数追加，不走 stdin。空串被拒绝——codex 在此场景
/// 会等 stdin 输入，我们不应该让门客陷入这个假死状态。
///
/// 失败来源：binary 不在 PATH、stdout pipe 拿不到。
pub fn spawn_codex(cfg: &CodexLaunchConfig, prompt: &str) -> std::io::Result<SpawnedCodex> {
    if prompt.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "codex exec requires a non-empty prompt (it is the positional argv, not stdin)",
        ));
    }

    let mut cmd = Command::new(&cfg.binary);
    let mut argv = cfg.build_args();
    argv.push(prompt.to_string());

    cmd.args(&argv)
        // codex 对 stdin 做了「如果是 TTY 或 pipe 则读一次」的逻辑——
        // 我们把 stdin 直接关成 null，避免出现「Reading additional input from stdin...」
        // 无尽等待的假死。
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // codex 往 stderr 吐 warning（"Reading additional input from stdin..." 等），
        // 继承到父进程方便人肉排障；真正的事件流在 stdout 的 JSONL 里。
        .stderr(Stdio::inherit())
        // kill_on_drop 防止测试进程退出后 codex 变孤儿进程。
        .kill_on_drop(true);

    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }

    tracing::info!(
        binary = %cfg.binary,
        model = %cfg.model,
        argv_count = argv.len(),
        "spawning codex exec subprocess"
    );

    let mut child = cmd.spawn()?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("codex child missing stdout pipe"))?;

    Ok(SpawnedCodex { child, stdout, pid })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_with_empty_prompt_errors() {
        let cfg = CodexLaunchConfig {
            model: "m".into(),
            ..Default::default()
        };
        let err = match spawn_codex(&cfg, "") {
            Ok(_) => panic!("empty prompt should fail"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn spawn_with_bogus_binary_errors() {
        let cfg = CodexLaunchConfig {
            binary: "/definitely/not/a/real/binary/codex".to_string(),
            model: "m".into(),
            ..Default::default()
        };
        assert!(spawn_codex(&cfg, "hi").is_err());
    }
}
