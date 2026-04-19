//! 起一个真实的 `claude` 子进程并返回它的 stdio 句柄。
//!
//! 为什么把 spawn 从 Agent 里拆出来：
//! 1. spawn 涉及 argv/env/cwd 的细节，单独写更好测（起一个 `/bin/cat` 替身
//!    也能验证 stdio wiring）；
//! 2. Agent 层只关心 stdin/stdout，不关心 PID——失败路径更清晰。

use crate::config::CcLaunchConfig;
use std::process::Stdio;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// `spawn_claude` 的返回值。成功后 child 正在后台跑，stdin/stdout 已 pipe。
pub struct SpawnedCc {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub pid: Option<u32>,
}

/// 启动 `claude` 子进程——headless stream-json 模式。
///
/// 失败原因主要两类：
/// 1. `claude` 不在 PATH（或 `binary` 指错）——返回 `io::Error`;
/// 2. stdin/stdout pipe 拿不到——理论上不该发生，但防御性检查。
pub fn spawn_claude(cfg: &CcLaunchConfig) -> std::io::Result<SpawnedCc> {
    let mut cmd = Command::new(&cfg.binary);
    cmd.args(cfg.build_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // cc 会往 stderr 吐 verbose 日志；继承到父进程方便人肉排障。
        // 真正的事件流在 stdout 的 stream-json 里。
        .stderr(Stdio::inherit())
        // kill_on_drop 防止测试进程退出后 cc 变孤儿进程。
        .kill_on_drop(true);

    // 清 CLAUDECODE / CLAUDE_CODE_* —— 我们自己如果在 Claude Code 会话里运行，
    // 子 cc 看到这些 env 会触发嵌套检测进入卡住状态（实测 30s 不反连）。
    // 参照 anya/apps/server/src/broker/backends/claude-code-backend.ts:307-326。
    cmd.env_remove("CLAUDECODE");
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE_CODE_") {
            cmd.env_remove(&k);
        }
    }

    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }

    tracing::info!(
        binary = %cfg.binary,
        model = %cfg.model,
        "spawning claude headless subprocess"
    );

    let mut child = cmd.spawn()?;
    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("claude child missing stdin pipe"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("claude child missing stdout pipe"))?;

    Ok(SpawnedCc {
        child,
        stdin,
        stdout,
        pid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 `cat` 替身验证 stdio 搭桥——不需要真 claude。
    /// cat 会把 stdin 回显到 stdout，模拟 stream-json pass-through。
    #[tokio::test]
    async fn spawn_with_cat_stub_yields_usable_pipes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let cfg = CcLaunchConfig {
            binary: "cat".to_string(),
            // cat 不认 claude 的 flag，改用 extra_args 给它一个空参数集。
            // 但 build_args 固定会加一堆 --bare etc——cat 忽略未识别参数会报错，
            // 所以这里用最小 cfg 直接 spawn 而不是 spawn_claude。
            ..Default::default()
        };
        // 绕开 build_args：用原生 Command 而不是 spawn_claude，单纯验 pipe 成立。
        let mut child = Command::new(&cfg.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cat exists");
        let mut stdin = child.stdin.take().expect("stdin");
        let mut stdout = child.stdout.take().expect("stdout");
        stdin.write_all(b"hello\n").await.unwrap();
        drop(stdin);
        let mut buf = String::new();
        stdout.read_to_string(&mut buf).await.unwrap();
        assert_eq!(buf, "hello\n");
        let _ = child.wait().await;
    }

    #[test]
    fn spawn_with_bogus_binary_errors() {
        let cfg = CcLaunchConfig {
            binary: "/definitely/not/a/real/binary/claude".to_string(),
            ..Default::default()
        };
        assert!(spawn_claude(&cfg).is_err());
    }
}
