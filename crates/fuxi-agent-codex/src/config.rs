//! `codex` 子进程启动配置。
//!
//! 为什么独立成 struct：启动参数（model/cwd/sandbox 策略）会随 profile 变化，
//! 集中在一处便于编排层（玄女）覆写；而 `--json` 等稳定 flag 在 `build_args`
//! 里硬塞，避免调用点漏传。

use std::path::PathBuf;

/// 默认回落模型——通过环境变量 `FUXI_CODEX_MODEL` 覆盖。
/// 不写死，是因为 codex 的可用模型与登录账号类型强相关
/// （ChatGPT 账号 vs API key），硬编码会导致不同环境初始化失败。
pub const DEFAULT_MODEL_ENV: &str = "FUXI_CODEX_MODEL";
/// 空串 = 不传 `-m` flag，让 codex 按登录账号自选默认模型。
///
/// 2026-04-20 用户复测发现：硬编码 `gpt-5.1-mini` 在 ChatGPT 账号
/// auth 下会被 codex 拒 `invalid_request_error`，进程 exit 但因为
/// 没有进程 exit 检测 → 没 AgentDead → 用户看起来卡住 4 分钟。
/// CLAUDE.md 里早记过这坑，这轮把默认改空让 codex 自选。走 API key
/// 的用户显式 `export FUXI_CODEX_MODEL=<可用模型>` 即可。
pub const DEFAULT_MODEL_FALLBACK: &str = "";

/// `codex exec` 启动参数。
///
/// 注意：codex 的 prompt 是**位置参数**，不走 stdin。因此 config 不保存 prompt——
/// 它由 `CodexAgent::dispatch` 从 Task 里组装，spawn 时作为 argv 尾部追加。
#[derive(Debug, Clone)]
pub struct CodexLaunchConfig {
    /// `-m <model>`。空字符串意味着「使用 codex 默认」——不传 `-m` flag。
    pub model: String,
    /// `-C <dir>` 启动 cwd。门客跑起来后应指向 worktree。
    pub cwd: Option<PathBuf>,
    /// `--full-auto`——低摩擦沙盒执行（on-request 审批、workspace-write 沙盒）。
    /// 和 `bypass_approvals` 互斥；若两者同时为 true，`build_args` 会只写
    /// bypass（更宽松者胜出）以避免 codex-cli 0.118 的 `cannot be used with` 报错。
    pub full_auto: bool,
    /// `--dangerously-bypass-approvals-and-sandbox`——无人值守场景用。
    /// 危险性外部已知，上层是否开启由编排层决定。
    pub bypass_approvals: bool,
    /// 额外原样透传 flag，供未覆盖到的场景（`--skip-git-repo-check` 等）。
    pub extra_args: Vec<String>,
    /// 可执行文件路径——默认 `"codex"`（PATH 查找）。
    pub binary: String,
}

impl Default for CodexLaunchConfig {
    fn default() -> Self {
        Self {
            model: resolve_default_model(),
            cwd: None,
            full_auto: true,
            bypass_approvals: true,
            extra_args: Vec::new(),
            binary: "codex".to_string(),
        }
    }
}

impl CodexLaunchConfig {
    /// 组装 argv（不含 binary，也不含末尾 prompt——调用方 spawn 单独传）。
    ///
    /// 硬编码的稳定 flag：
    /// - `exec` 子命令
    /// - `--json` 开启 JSONL 事件输出（我们唯一认识的 wire 格式）
    ///
    /// 互斥处理：`full_auto` 与 `bypass_approvals` 同真时，后者优先；
    /// 这是 codex-cli 的 argparse 约束，不是我们的策略选择。
    pub fn build_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec!["exec".to_string(), "--json".to_string()];

        if self.bypass_approvals {
            args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        } else if self.full_auto {
            args.push("--full-auto".to_string());
        }

        if !self.model.is_empty() {
            args.push("-m".to_string());
            args.push(self.model.clone());
        }

        if let Some(cwd) = &self.cwd {
            args.push("-C".to_string());
            args.push(cwd.to_string_lossy().into_owned());
        }

        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// 读 `FUXI_CODEX_MODEL`，否则退回 `DEFAULT_MODEL_FALLBACK`。
pub fn resolve_default_model() -> String {
    std::env::var(DEFAULT_MODEL_ENV).unwrap_or_else(|_| DEFAULT_MODEL_FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_contain_exec_and_json() {
        let cfg = CodexLaunchConfig {
            model: "gpt-5.1-mini".to_string(),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert_eq!(args[0], "exec");
        assert!(args.iter().any(|a| a == "--json"));
        assert!(args.iter().any(|a| a == "-m"));
    }

    /// bypass 优先：同时开启时只应看到 bypass，不应看到 full-auto
    /// （codex-cli 会拒绝两个同时出现）。
    #[test]
    fn bypass_and_full_auto_are_mutually_exclusive() {
        let cfg = CodexLaunchConfig {
            model: "m".into(),
            full_auto: true,
            bypass_approvals: true,
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(
            args.iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox")
        );
        assert!(!args.iter().any(|a| a == "--full-auto"));
    }

    #[test]
    fn full_auto_only_when_bypass_false() {
        let cfg = CodexLaunchConfig {
            model: "m".into(),
            full_auto: true,
            bypass_approvals: false,
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(args.iter().any(|a| a == "--full-auto"));
        assert!(
            !args
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox")
        );
    }

    #[test]
    fn empty_model_skips_model_flag() {
        let cfg = CodexLaunchConfig {
            model: String::new(),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(!args.iter().any(|a| a == "-m"));
    }

    #[test]
    fn cwd_emits_minus_capital_c() {
        let cfg = CodexLaunchConfig {
            model: "m".into(),
            cwd: Some(PathBuf::from("/tmp/foo")),
            ..Default::default()
        };
        let args = cfg.build_args();
        let idx = args.iter().position(|a| a == "-C").expect("flag present");
        assert_eq!(args[idx + 1], "/tmp/foo");
    }

    #[test]
    fn extra_args_are_appended_last() {
        let cfg = CodexLaunchConfig {
            model: "m".into(),
            extra_args: vec!["--skip-git-repo-check".into()],
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(args.iter().any(|a| a == "--skip-git-repo-check"));
    }

    #[test]
    fn default_binary_is_codex() {
        let cfg = CodexLaunchConfig::default();
        assert_eq!(cfg.binary, "codex");
    }

    #[test]
    fn neither_sandbox_flag_when_both_false() {
        let cfg = CodexLaunchConfig {
            model: "m".into(),
            full_auto: false,
            bypass_approvals: false,
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(!args.iter().any(|a| a == "--full-auto"));
        assert!(
            !args
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox")
        );
    }
}
