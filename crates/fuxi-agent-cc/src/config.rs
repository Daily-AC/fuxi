//! `claude` 子进程启动配置。
//!
//! 为什么独立成 struct：启动参数是编排层（玄女）会反复调整的变量——模型、
//! cwd、allowed_tools、append_system_prompt 都可能随 profile 变化。而那些
//! 「几乎必须」的稳定 flag（--bare / --print / --*-format stream-json /
//! --permission-mode bypassPermissions）集中在 `Default` 里，避免每个调用点
//! 重复手抄、出错。

use std::path::PathBuf;

/// 默认回落模型——P1 阶段全部用 haiku 压成本。通过环境变量
/// `FUXI_CC_MODEL` 覆盖，不写死在代码里。
pub const DEFAULT_MODEL_ENV: &str = "FUXI_CC_MODEL";
pub const DEFAULT_MODEL_FALLBACK: &str = "haiku";

/// `claude` headless 启动参数。任何字段都可以不填——`Default` 给出最
/// 稳定的一套（见 `reference_cc_stream_json.md`）。
#[derive(Debug, Clone)]
pub struct CcLaunchConfig {
    /// `--model <name>`，如 `"haiku"` / `"sonnet"` / `"opus"`.
    pub model: String,
    /// 启动时 `cwd`。`None` = 继承父进程。门客真正跑起来后应指向其 worktree。
    pub cwd: Option<PathBuf>,
    /// `--append-system-prompt`：给角色 profile 留的接口。
    pub append_system_prompt: Option<String>,
    /// `--allowed-tools Tool1,Tool2`。`None` = 不限。
    pub allowed_tools: Option<Vec<String>>,
    /// 额外原样透传的 flag，供罕见场景，不走任何校验。
    pub extra_args: Vec<String>,
    /// 可执行文件路径——默认 `"claude"`（靠 PATH 查找）。
    /// 为什么不默认带 alias 里的 `--dangerously-skip-permissions`：
    /// alias 是 shell-only 的，我们 spawn 直接走 argv，显式传递所有 flag，
    /// 不要隐式依赖用户 shell 环境。
    pub binary: String,
}

impl Default for CcLaunchConfig {
    fn default() -> Self {
        Self {
            model: resolve_default_model(),
            cwd: None,
            append_system_prompt: None,
            allowed_tools: None,
            extra_args: Vec::new(),
            binary: "claude".to_string(),
        }
    }
}

impl CcLaunchConfig {
    /// 构建 `claude` 的 argv。不包括 `binary` 自身——调用方（spawn）单独传。
    ///
    /// 默认开启的稳定 flag 集：
    /// - `--bare`：跳 hooks / plugin sync / CLAUDE.md discovery，否则噪音爆炸
    /// - `--print`：headless 模式，读 stdin、写 stdout 退出
    /// - `--input-format stream-json` / `--output-format stream-json`：
    ///   双向结构化协议，才是我们要的 wire format
    /// - `--verbose`：在 stream-json 模式下才会吐出每一条事件（init、assistant
    ///   的 thinking 等），不是可选
    /// - `--permission-mode bypassPermissions` + `--dangerously-skip-permissions`：
    ///   P1 跑不起来不如不跑。玄女层面再收拢风险。
    /// - `--no-session-persistence`：避免污染用户本机 `claude` 会话历史
    pub fn build_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--bare".to_string(),
            "--print".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--no-session-persistence".to_string(),
            "--model".to_string(),
            self.model.clone(),
        ];

        if let Some(prompt) = &self.append_system_prompt {
            args.push("--append-system-prompt".to_string());
            args.push(prompt.clone());
        }

        if let Some(tools) = &self.allowed_tools
            && !tools.is_empty()
        {
            args.push("--allowed-tools".to_string());
            args.push(tools.join(","));
        }

        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// 读 `FUXI_CC_MODEL`，否则退回 `DEFAULT_MODEL_FALLBACK`。
pub fn resolve_default_model() -> String {
    std::env::var(DEFAULT_MODEL_ENV).unwrap_or_else(|_| DEFAULT_MODEL_FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_contain_stable_flags() {
        let cfg = CcLaunchConfig {
            // 手动指定，避免 env 干扰单测。
            model: "haiku".to_string(),
            ..Default::default()
        };
        let args = cfg.build_args();
        for flag in [
            "--bare",
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "--verbose",
            "--permission-mode",
            "bypassPermissions",
            "--dangerously-skip-permissions",
            "--no-session-persistence",
            "--model",
            "haiku",
        ] {
            assert!(
                args.iter().any(|a| a == flag),
                "missing flag {flag:?} in {args:?}"
            );
        }
    }

    #[test]
    fn append_system_prompt_flows_through() {
        let cfg = CcLaunchConfig {
            model: "haiku".to_string(),
            append_system_prompt: Some("role: pm".to_string()),
            ..Default::default()
        };
        let args = cfg.build_args();
        let idx = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("flag present");
        assert_eq!(args[idx + 1], "role: pm");
    }

    #[test]
    fn allowed_tools_joined_with_comma() {
        let cfg = CcLaunchConfig {
            model: "haiku".to_string(),
            allowed_tools: Some(vec!["Read".into(), "Edit".into()]),
            ..Default::default()
        };
        let args = cfg.build_args();
        let idx = args
            .iter()
            .position(|a| a == "--allowed-tools")
            .expect("flag present");
        assert_eq!(args[idx + 1], "Read,Edit");
    }

    #[test]
    fn empty_allowed_tools_is_skipped() {
        let cfg = CcLaunchConfig {
            model: "haiku".to_string(),
            allowed_tools: Some(vec![]),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(!args.iter().any(|a| a == "--allowed-tools"));
    }

    #[test]
    fn extra_args_are_appended() {
        let cfg = CcLaunchConfig {
            model: "haiku".to_string(),
            extra_args: vec!["--include-partial-messages".into()],
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(args.iter().any(|a| a == "--include-partial-messages"));
    }

    #[test]
    fn default_binary_is_claude() {
        let cfg = CcLaunchConfig {
            model: "haiku".to_string(),
            ..Default::default()
        };
        assert_eq!(cfg.binary, "claude");
    }
}
