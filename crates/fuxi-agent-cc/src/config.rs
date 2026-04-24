//! `claude` 子进程启动配置。
//!
//! 为什么独立成 struct：启动参数是编排层（玄女）会反复调整的变量——模型、
//! cwd、allowed_tools、append_system_prompt 都可能随 profile 变化。而那些
//! 「几乎必须」的稳定 flag（--bare / --print / --*-format stream-json /
//! --permission-mode bypassPermissions）集中在 `Default` 里，避免每个调用点
//! 重复手抄、出错。

use std::path::PathBuf;

/// 环境变量：`FUXI_CC_MODEL`。若设置则透传 `--model $ENV`；未设置默认 `haiku`。
///
/// WHY：当前阶段以成本优先，先用 haiku 跑门客回路；后续可按场景切回更强模型。
pub const DEFAULT_MODEL_ENV: &str = "FUXI_CC_MODEL";
pub const DEFAULT_MODEL_FALLBACK: &str = "haiku";

/// `claude` headless 启动参数。任何字段都可以不填——`Default` 给出最
/// 稳定的一套（见 `reference_cc_stream_json.md`）。
#[derive(Debug, Clone)]
pub struct CcLaunchConfig {
    /// `--model <name>`——`None` 时**不传** `--model`，走 cc 默认。
    /// 如 `Some("haiku")` / `Some("sonnet")` / `Some("opus")`。
    pub model: Option<String>,
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
    /// `--sdk-url ws://...`——v0.1 薄片 H：启用 WS 反连模式。`None` 时走
    /// 传统 stdio 模式（仅单测 fixtures 还会用到）；生产路径**必传**。
    pub sdk_url: Option<String>,
    /// `--resume <id>`——续写之前那次 cc session（M1.1 策府：门客记忆走 cc 原生）。
    /// 与 `session_id` **互斥**：同时 Some 时 `resume_session_id` 生效、`session_id` 被忽略。
    pub resume_session_id: Option<String>,
    /// `--session-id <uuid>`——新起 session 时指定 id（以便后续 `--resume` 精准续写）。
    /// 和 `resume_session_id` 互斥。
    pub session_id: Option<String>,
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
            sdk_url: None,
            resume_session_id: None,
            session_id: None,
        }
    }
}

impl CcLaunchConfig {
    /// 构建 `claude` 的 argv。不包括 `binary` 自身——调用方（spawn）单独传。
    ///
    /// 默认开启的稳定 flag 集：
    /// - `--print`：headless 模式，读 stdin、写 stdout 退出
    /// - `--input-format stream-json` / `--output-format stream-json`：
    ///   双向结构化协议，才是我们要的 wire format
    /// - `--verbose`：在 stream-json 模式下才会吐出每一条事件（init、assistant
    ///   的 thinking 等），不是可选
    /// - `--permission-mode bypassPermissions` + `--dangerously-skip-permissions`：
    ///   P1 跑不起来不如不跑。玄女层面再收拢风险。
    /// - `--no-session-persistence`：避免污染用户本机 `claude` 会话历史
    ///
    /// 特别说明：**不启用 `--bare`**。memo 里写「bare 几乎必须」是针对
    /// 用户本机 hooks 噪音的优化，但实测 `--bare` 会跳过 keychain 读取、
    /// 导致 cc 进入「Not logged in」状态——对 P1 的 E2E 验证是阻塞性问题。
    /// 等 `use_bare` 开关需要时（noisy hooks + 独立 token 注入）再单独加。
    pub fn build_args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();

        // WS 反连：`--sdk-url` 必须**最前面**（参照 anya claude-code-backend.ts:255）；
        // claude CLI 看到它就进 SDK 模式，NDJSON 通过 WS 双向走，stdin/stdout 不再是
        // wire 通道（stderr 仍是 verbose 日志）。
        if let Some(url) = &self.sdk_url {
            args.push("--sdk-url".to_string());
            args.push(url.clone());
        }

        args.extend([
            "--print".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "--dangerously-skip-permissions".to_string(),
        ]);

        // `--no-session-persistence` 只在**不需要续写**时启用——它关掉的是 cc
        // 把 session 落到 `~/.claude/projects/` 的行为。策府路径要持久化 session 才
        // 能后续 `--resume`，所以只要 `session_id` 或 `resume_session_id` 任一被设，
        // 就不加这个 flag。保持历史行为：默认 demo 场景（两者都 None）仍然不污染用户库。
        let needs_persistence = self.session_id.is_some() || self.resume_session_id.is_some();
        if !needs_persistence {
            args.push("--no-session-persistence".to_string());
        }

        if let Some(model) = &self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

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

        // 策府 / cc --resume：续写老 session。resume_session_id 优先于 session_id
        // （两者互斥；用户把两个都填上时我们不会失败，但只带一条语义到 CLI）。
        if let Some(sid) = &self.resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.clone());
        } else if let Some(sid) = &self.session_id {
            args.push("--session-id".to_string());
            args.push(sid.clone());
        }

        args.extend(self.extra_args.iter().cloned());

        // SDK 模式下 claude 仍需要 `-p <prompt>` 才进 headless（空串占位，
        // 真正的 prompt 走 WS `{type:"user",...}` 消息）。参照 anya:278。
        if self.sdk_url.is_some() {
            args.push("-p".to_string());
            args.push(String::new());
        }

        args
    }
}

/// 读 `FUXI_CC_MODEL`；未设回退到 `haiku`。
pub fn resolve_default_model() -> Option<String> {
    std::env::var(DEFAULT_MODEL_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(DEFAULT_MODEL_FALLBACK.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_contain_stable_flags() {
        let cfg = CcLaunchConfig {
            // 手动指定，避免 env 干扰单测。
            model: Some("haiku".to_string()),
            ..Default::default()
        };
        let args = cfg.build_args();
        for flag in [
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

    /// 默认 **不** 传 `--bare`：memo 里写「bare 几乎必须」但实测会断 keychain，
    /// 导致 cc 认为未登录。回归兜底，避免改错方向。
    #[test]
    fn default_args_do_not_include_bare() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            ..Default::default()
        };
        assert!(!cfg.build_args().iter().any(|a| a == "--bare"));
    }

    #[test]
    fn append_system_prompt_flows_through() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
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
            model: Some("haiku".to_string()),
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
            model: Some("haiku".to_string()),
            allowed_tools: Some(vec![]),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(!args.iter().any(|a| a == "--allowed-tools"));
    }

    #[test]
    fn extra_args_are_appended() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            extra_args: vec!["--include-partial-messages".into()],
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(args.iter().any(|a| a == "--include-partial-messages"));
    }

    #[test]
    fn default_binary_is_claude() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.binary, "claude");
    }

    /// WS 反连模式：`--sdk-url` 必须最前、末尾有 `-p ""`。
    #[test]
    fn sdk_url_is_first_and_adds_placeholder_prompt() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            sdk_url: Some("ws://127.0.0.1:12345/ws/cli/abc".into()),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert_eq!(args[0], "--sdk-url");
        assert_eq!(args[1], "ws://127.0.0.1:12345/ws/cli/abc");
        let n = args.len();
        assert_eq!(args[n - 2], "-p");
        assert_eq!(args[n - 1], "");
    }

    /// 没启 WS 模式时不应插入 `--sdk-url` 或 `-p ""`。
    #[test]
    fn stdio_mode_omits_sdk_url_and_placeholder() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            sdk_url: None,
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(!args.iter().any(|a| a == "--sdk-url"));
        // -p "" 只在 SDK 模式下追加；stdio 下 build_args 尾部不应是 "-p"
        assert_ne!(args.last().map(String::as_str), Some("-p"));
    }

    /// 策府：`resume_session_id` 透传 `--resume <id>`。
    #[test]
    fn resume_session_id_emits_resume_flag() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            resume_session_id: Some("abc-123".into()),
            ..Default::default()
        };
        let args = cfg.build_args();
        let idx = args
            .iter()
            .position(|a| a == "--resume")
            .expect("--resume flag present");
        assert_eq!(args[idx + 1], "abc-123");
        // 默认不带 session_id；不该同时出现 --session-id
        assert!(!args.iter().any(|a| a == "--session-id"));
    }

    /// 新开 session 指定 id 走 `--session-id`。
    #[test]
    fn session_id_emits_session_id_flag_when_no_resume() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            session_id: Some("new-sess-1".into()),
            ..Default::default()
        };
        let args = cfg.build_args();
        let idx = args
            .iter()
            .position(|a| a == "--session-id")
            .expect("--session-id flag present");
        assert_eq!(args[idx + 1], "new-sess-1");
        assert!(!args.iter().any(|a| a == "--resume"));
    }

    /// 两者同时给——resume_session_id 优先。
    #[test]
    fn resume_overrides_session_id_when_both_set() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            resume_session_id: Some("resume-1".into()),
            session_id: Some("new-1".into()),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(args.iter().any(|a| a == "--resume"));
        assert!(!args.iter().any(|a| a == "--session-id"));
    }

    /// `--no-session-persistence` 和 resume 互斥——cc 文档明写「sessions 未落盘
    /// 时不能 resume」。策府路径要求两者之一被设时就别加这个 flag。
    #[test]
    fn no_session_persistence_dropped_when_resume_or_session_id_set() {
        for cfg in [
            CcLaunchConfig {
                model: Some("haiku".into()),
                resume_session_id: Some("r-1".into()),
                ..Default::default()
            },
            CcLaunchConfig {
                model: Some("haiku".into()),
                session_id: Some("s-1".into()),
                ..Default::default()
            },
        ] {
            let args = cfg.build_args();
            assert!(
                !args.iter().any(|a| a == "--no-session-persistence"),
                "期望不含 --no-session-persistence；实际 args: {args:?}"
            );
        }
    }

    /// 都没给时不插入任何 resume-related flag。
    #[test]
    fn no_resume_flags_when_both_unset() {
        let cfg = CcLaunchConfig {
            model: Some("haiku".to_string()),
            ..Default::default()
        };
        let args = cfg.build_args();
        assert!(!args.iter().any(|a| a == "--resume"));
        assert!(!args.iter().any(|a| a == "--session-id"));
    }

    /// 未设 FUXI_CC_MODEL 时，默认回落到 haiku 并显式透传 `--model`。
    #[test]
    fn default_uses_haiku_when_env_absent() {
        // 保守清掉 env 避免干扰
        unsafe {
            std::env::remove_var(DEFAULT_MODEL_ENV);
        }
        let cfg = CcLaunchConfig::default();
        assert_eq!(cfg.model.as_deref(), Some(DEFAULT_MODEL_FALLBACK));
        let args = cfg.build_args();
        let idx = args
            .iter()
            .position(|a| a == "--model")
            .expect("expected --model flag");
        assert!(
            args.get(idx + 1)
                .is_some_and(|v| v == DEFAULT_MODEL_FALLBACK),
            "expected fallback model {DEFAULT_MODEL_FALLBACK}, got: {args:?}"
        );
    }
}
