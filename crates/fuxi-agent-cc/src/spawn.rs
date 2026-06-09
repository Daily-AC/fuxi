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

    apply_cc_env(&mut cmd, cfg);

    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }

    tracing::info!(
        binary = %cfg.binary,
        model = ?cfg.model,
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

/// cc 子进程环境变量处理——抽出独立 fn 便于单测（验 strip + extra_env 注入互不干扰）。
///
/// 顺序要点：先 strip（嵌套检测）→ 再注 NO_PROXY → **最后**注 extra_env。extra_env
/// 放最后保证不被 strip 循环（按 CLAUDECODE/CLAUDE_CODE_ 前缀删）误删；调用方自律
/// 不用那两个前缀当 extra_env 键。
fn apply_cc_env(cmd: &mut Command, cfg: &CcLaunchConfig) {
    // ── 两坑都要避 ──
    // 坑 1：嵌套检测。我们自己若在 Claude Code 会话里运行，子 cc 继承 env 会触发
    //        嵌套检测进入卡住状态。参照 sia/src/core/cc-process.ts:cleanEnv 和
    //        anya/apps/server/src/broker/backends/claude-code-backend.ts:307-326。
    cmd.env_remove("CLAUDECODE");
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE_CODE_")
            && !matches!(
                k.as_str(),
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS"
                    | "CLAUDE_CODE_USE_BEDROCK"
                    | "CLAUDE_CODE_USE_VERTEX"
            )
        {
            cmd.env_remove(&k);
        }
    }
    // 坑 2：Clash/Surge TUN 代理可能把 127.0.0.1 也代理走，导致 cc 反连 WS 时
    //        TCP SYN 被拦。**实测本机不设 NO_PROXY → --sdk-url 30s 永远 timeout**。
    //        参照 sia/src/core/cc-process.ts:666-667（它就是踩过这个坑）。
    cmd.env("NO_PROXY", "127.0.0.1,localhost");
    cmd.env("no_proxy", "127.0.0.1,localhost");

    // 块5：白名单式注入 extra_env（如玄女分身的 FUXI_TOPIC）。strip 循环只删
    // CLAUDECODE/CLAUDE_CODE_ 前缀继承 env，不碰这里显式 set 的键。子进程（含玄女
    // shell 的 fuxi dispatch）继承 → 默认带 --topic。
    for (k, v) in &cfg.extra_env {
        cmd.env(k, v);
    }
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

    /// 块5：apply_cc_env 把 extra_env（如 FUXI_TOPIC）注进子进程，且**不**被嵌套检测
    /// 的 strip 循环误删；同时验证继承的 CLAUDE_CODE_* 仍被 strip。用 `printenv` 打全
    /// 量 env 断言。
    #[tokio::test]
    async fn apply_cc_env_injects_extra_env_and_strips_claude_code() {
        // 父进程设一个 CLAUDE_CODE_ 变量——子进程应看不到（被 strip）。
        // SAFETY: 本测串行设/删自有 env，不与并发测试共享键。
        unsafe {
            std::env::set_var("CLAUDE_CODE_TESTSTRIP", "should-be-removed");
        }
        let cfg = CcLaunchConfig {
            binary: "printenv".to_string(),
            extra_env: vec![("FUXI_TOPIC".to_string(), "t-7-uuid".to_string())],
            ..Default::default()
        };
        let mut cmd = Command::new(&cfg.binary);
        apply_cc_env(&mut cmd, &cfg);
        let out = cmd.output().await.expect("run printenv");
        let env_dump = String::from_utf8_lossy(&out.stdout);

        assert!(
            env_dump.lines().any(|l| l == "FUXI_TOPIC=t-7-uuid"),
            "extra_env 应注入子进程：\n{env_dump}"
        );
        assert!(
            !env_dump.contains("CLAUDE_CODE_TESTSTRIP"),
            "继承的 CLAUDE_CODE_* 应被 strip：\n{env_dump}"
        );
        assert!(
            env_dump
                .lines()
                .any(|l| l == "NO_PROXY=127.0.0.1,localhost"),
            "NO_PROXY 应注入：\n{env_dump}"
        );

        unsafe {
            std::env::remove_var("CLAUDE_CODE_TESTSTRIP");
        }
    }
}
