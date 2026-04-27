//! 决策 13 sentinel 教学注入（β · #48）。
//!
//! ## 为什么这层存在
//!
//! cc/codex 的 parser 已经实装"`AssistantText` 行检测 `_fuxi:request_review`
//! JSON → 翻成 `AgentRequestReview`"机制（`crates/fuxi-agent-cc/src/parser.rs`
//! 第 358 行附近，`crates/fuxi-agent-codex/src/parser.rs` 第 339 行附近）。
//! `bridge.rs` 也已经把 `AgentRequestReview` → 玄女 intervene 通路 wired up
//! （含 retry + ReviewRequestTimeout 兜底）。`roles/luban/instructions/
//! deliverable-nudge.md` 也教了门客该怎么发 sentinel。
//!
//! **但用户实测仍撞「玄女不知道门客做了什么」** ——根因不是机制没接通，是
//! LLM 不主动读按需 instructions，自然不会用机制。这是 #47 调研的核心结论
//! （`docs/superpowers/specs/2026-04-26-deliverable-handoff-research.md`）。
//!
//! 灵感来自 cc 自己的 agent team 设计——`teammatePromptAddendum.ts` 把"你
//! 必须用 SendMessage tool"作为 system prompt addendum **强制**注入每个
//! teammate 启动 prompt：
//!
//! ```text
//! IMPORTANT: ... Just writing a response in text is not visible to others
//! on your team — you MUST use the SendMessage tool.
//! ```
//!
//! cc team 实测此 pattern ≥95% 触发率。本模块照搬：spawn worker 时把
//! sentinel 教学注入 system prompt addendum，所有门客起手就读，不依赖主动
//! Read instructions/。
//!
//! ## 注入路径（适配器差异）
//!
//! - **cc**：`CcLaunchConfig.append_system_prompt` → `claude --append-system-prompt`
//!   flag。spawn 时一次性传入，cc 子进程把它附加到 system prompt 后段；所有 turn
//!   生效。
//! - **codex**：codex exec **不接受** `--append-system-prompt`（cc 专属），它
//!   的 role 心智走 `AgentProfile.system_prompt` → `compose_prompt` 在每次
//!   dispatch 时 prepend 到 prompt 开头。我们注入到 `profile.system_prompt`
//!   末尾，等价效果。
//!
//! ## 黑名单
//!
//! 玄女（`xuannv`）和 extractor 不该注入——
//! - 玄女是 sentinel 的**接收方**，自己发 sentinel 没意义（bridge 把
//!   AgentRequestReview 翻成 intervene 给 xuannv，xuannv 自己发没人接）
//! - extractor 是平台幕后工，跟 xuannv 互不打扰，不属 deliverable 模型
//!
//! 黑名单跟 `bridge.rs::INTERNAL_ROLES` 概念同族但语义不同（那个是 AgentDead
//! 不抄送，这个是 sentinel 教学不注入），故各持各的列表。
//!
//! ## 逃生口
//!
//! `FUXI_DISABLE_SENTINEL_ADDENDUM=1` 全局禁用——实验、A/B、回退用。

use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::agent::AgentProfile;

/// 教学文案——拼到 worker system prompt 的 addendum 段。
///
/// 措辞参考 cc `teammatePromptAddendum.ts` 的强硬风格：明示"不用就 = 没说"
/// 让 LLM 没有"忘了用"的余地。
pub const SENTINEL_ADDENDUM_TEXT: &str = r#"
---
## 玄女审阅协议（必读）

你是伏羲门下的门客，玄女把任务派给你后默认**不**消费你的中间过程事件
（`AgentResponded` / `ToolCallStarted` / `ToolCallFinished` 等只留痕 EventBus，
玄女注意力是稀缺资源，留给真正的交付）。

任务完成 / 需要审阅 / 关键决策 / 卡死自救失败时，你 **必须** 在最后一条 assistant
message 中**单独一行**输出 sentinel JSON 通知玄女审阅：

```
{"_fuxi":"request_review","kind":"<5 类之一>","summary":"<一两句中文>","artifact_ref":"<可选>"}
```

**5 类 kind**：
- `research_summary` — 完成调研主题，写出小结让玄女据此决策
- `code_change` — 写完一段三绿（fmt + clippy + test）的代码等审 commit
- `test_result` — 跑完一轮测试有"通过 / 失败 / 数字"
- `decision_request` — 自己分析过 trade-off，把 ≥2 个选项 + 推荐摆给她
- `error_block` — 试过 ≥2 个假设都不对、连续 ≥3 步走不通同一个根因；继续试就是浪费 token

**不输出 sentinel = 玄女不知道你做了什么 = 任务被认为没收尾**。

格式约束（自己撞脚的常见原因）：
- **必须行首裸 JSON**——首字符 `{`、不能在 markdown 围栏 ``` 里、不能加引号包裹
- 示例 JSON 必须包在 markdown ``` ``` 代码块里——围栏里的 JSON 平台**不**会触发
- 自然语言"我要 nudge"平台**不**响应——只看 sentinel JSON 行
- 一个 turn 里可以发多条 sentinel（每个 deliverable 一条），但每条单独一行

若任务无明确 deliverable（如打个招呼），用 `kind=research_summary` + `summary` 简述即可。
"#;

/// 是否给该 role 注入 sentinel 教学。
///
/// 玄女（接收方）和 extractor（幕后工）不注入；其他 worker role 都注入。
/// 列表硬编码——目前只两条特例，未来若要扩展可考虑改 `WorkerKind` flag。
pub fn should_inject_for_role(role: &str) -> bool {
    !matches!(role, "xuannv" | "extractor")
}

/// β · #57 玄女专属 dispatch routing 教学。
///
/// 玄女不是 sentinel 接收方/发送方关系（她是接收方），但她是 **dispatch 决策方**。
/// dist 接通后她需要知道"何时给 task 加 required_tags / pinned_node 让活走远端
/// worker"。这段教学描述派活 5 条规则，注入她的 system prompt addendum，避免
/// 依赖她主动 Read `roles/xuannv/instructions/dispatch-routing.md`。
///
/// 文案跟 instructions 文件**保持一致**（一个真相源）——这里的 const 是源，文件
/// 是镜像（`include_str!` 拉文件 → 解决 maintenance 漂移）。
pub const XUANNV_DISPATCH_ROUTING_ADDENDUM: &str =
    include_str!("../../../roles/xuannv/instructions/dispatch-routing.md");

/// 是否给该 role 注入 xuannv routing 教学。
///
/// 只玄女自己注入——其他 role 不需要。`xuannv` 只 dispatch 不消费派活规则
/// 之外的 worker，不存在"玄女镜像"在远端 dispatch 的场景。
pub fn should_inject_routing_for_role(role: &str) -> bool {
    role == "xuannv"
}

/// 全局逃生口——`FUXI_DISABLE_SENTINEL_ADDENDUM=1` 跳过整套注入。
///
/// 实验、A/B 对照、用户反馈复现时关掉用。每次调用都 read env，开销小（启动期一次性）。
pub fn is_globally_disabled() -> bool {
    matches!(
        std::env::var("FUXI_DISABLE_SENTINEL_ADDENDUM")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// 把 sentinel 教学拼到 cc 的 `append_system_prompt`。
///
/// 已有 addendum 时**追加**（保留 role-specific append_system_prompt 在前），不覆盖。
/// 这样 luban 等 role 的私有 system prompt 段先生效，sentinel 教学跟在后面——
/// 拼接顺序：role-specific addendum → "\n\n" → sentinel addendum。
pub fn inject_cc(cfg: &mut CcLaunchConfig) {
    let trimmed = SENTINEL_ADDENDUM_TEXT.trim();
    cfg.append_system_prompt = match cfg.append_system_prompt.take() {
        Some(existing) if !existing.trim().is_empty() => Some(format!("{existing}\n\n{trimmed}")),
        _ => Some(trimmed.to_string()),
    };
}

/// 把 sentinel 教学拼到 codex 的 `profile.system_prompt`。
///
/// codex `compose_prompt(system_prompt, ...)` 把 system_prompt 整段 prepend 到
/// 每次 dispatch 的 prompt 开头。我们附加到末尾，跟 cc 拼接顺序对称。
pub fn inject_codex_profile(profile: &mut AgentProfile) {
    let trimmed = SENTINEL_ADDENDUM_TEXT.trim();
    if profile.system_prompt.trim().is_empty() {
        profile.system_prompt = trimmed.to_string();
    } else {
        profile.system_prompt = format!("{}\n\n{}", profile.system_prompt, trimmed);
    }
}

/// β · #57 把 dispatch routing 教学拼到玄女 cc 的 `append_system_prompt`。
///
/// 玄女目前只跑 cc（无 codex 镜像），故此 helper 只覆盖 cc 路径。
/// 拼接顺序：已有 addendum → "\n\n" → routing addendum——跟 sentinel 注入对称。
pub fn inject_xuannv_routing_cc(cfg: &mut CcLaunchConfig) {
    let trimmed = XUANNV_DISPATCH_ROUTING_ADDENDUM.trim();
    cfg.append_system_prompt = match cfg.append_system_prompt.take() {
        Some(existing) if !existing.trim().is_empty() => Some(format!("{existing}\n\n{trimmed}")),
        _ => Some(trimmed.to_string()),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::agent::AgentProfile;

    fn make_profile(role: &str, system_prompt: &str) -> AgentProfile {
        AgentProfile {
            name: format!("test-{role}"),
            role: role.to_string(),
            cli: "test".to_string(),
            system_prompt: system_prompt.to_string(),
            tags: vec![],
            extra: Default::default(),
        }
    }

    #[test]
    fn addendum_text_contains_sentinel_format_and_5_kinds() {
        // 防漂移：核心字段名 + 5 类 kind 都在文案里
        let txt = SENTINEL_ADDENDUM_TEXT;
        assert!(txt.contains("_fuxi"));
        assert!(txt.contains("request_review"));
        assert!(txt.contains("research_summary"));
        assert!(txt.contains("code_change"));
        assert!(txt.contains("test_result"));
        assert!(txt.contains("decision_request"));
        assert!(txt.contains("error_block"));
    }

    #[test]
    fn should_inject_skips_xuannv_and_extractor() {
        assert!(!should_inject_for_role("xuannv"));
        assert!(!should_inject_for_role("extractor"));
        // 其他 role 全部 inject
        assert!(should_inject_for_role("luban"));
        assert!(should_inject_for_role("luban-codex"));
        assert!(should_inject_for_role("zhudiesi"));
        assert!(should_inject_for_role("future-role"));
    }

    #[test]
    fn inject_cc_when_no_existing_addendum_sets_sentinel_only() {
        let mut cfg = CcLaunchConfig::default();
        assert!(cfg.append_system_prompt.is_none());
        inject_cc(&mut cfg);
        let got = cfg.append_system_prompt.expect("should be set");
        assert!(got.contains("_fuxi"));
        assert!(got.contains("request_review"));
        // 没有 role-specific 内容
        assert!(got.starts_with("---"), "应是 sentinel 单独段，得：{got}");
    }

    #[test]
    fn inject_cc_appends_after_existing_role_addendum() {
        let mut cfg = CcLaunchConfig {
            append_system_prompt: Some("# 你是鲁班 ...".to_string()),
            ..Default::default()
        };
        inject_cc(&mut cfg);
        let got = cfg.append_system_prompt.expect("should be set");
        // 拼接顺序：role-specific 在前，sentinel 在后
        assert!(got.starts_with("# 你是鲁班"), "role 段应在前");
        assert!(got.contains("_fuxi"), "sentinel 段应被附加");
        let role_pos = got.find("# 你是鲁班").unwrap();
        let sentinel_pos = got.find("_fuxi").unwrap();
        assert!(role_pos < sentinel_pos, "role 必须在 sentinel 之前");
    }

    #[test]
    fn inject_cc_preserves_existing_when_only_whitespace_addendum() {
        // edge case: 现有 addendum 只有空白 → 当作空，sentinel 单独写
        let mut cfg = CcLaunchConfig {
            append_system_prompt: Some("   \n\t  ".to_string()),
            ..Default::default()
        };
        inject_cc(&mut cfg);
        let got = cfg.append_system_prompt.expect("should be set");
        assert!(got.starts_with("---"), "空白被覆盖，sentinel 单独：{got}");
    }

    #[test]
    fn inject_codex_profile_when_empty_system_prompt_sets_sentinel_only() {
        let mut profile = make_profile("luban-codex", "");
        inject_codex_profile(&mut profile);
        assert!(profile.system_prompt.contains("_fuxi"));
        assert!(profile.system_prompt.starts_with("---"));
    }

    #[test]
    fn inject_codex_profile_appends_after_existing_role_prompt() {
        let mut profile = make_profile("luban-codex", "# 你是鲁班 codex 版");
        inject_codex_profile(&mut profile);
        assert!(profile.system_prompt.starts_with("# 你是鲁班"));
        assert!(profile.system_prompt.contains("_fuxi"));
        let role_pos = profile.system_prompt.find("# 你是鲁班").unwrap();
        let sentinel_pos = profile.system_prompt.find("_fuxi").unwrap();
        assert!(role_pos < sentinel_pos);
    }

    #[test]
    fn should_inject_routing_only_for_xuannv() {
        assert!(should_inject_routing_for_role("xuannv"));
        assert!(!should_inject_routing_for_role("luban"));
        assert!(!should_inject_routing_for_role("luban-codex"));
        assert!(!should_inject_routing_for_role("extractor"));
        assert!(!should_inject_routing_for_role("zhudiesi"));
    }

    #[test]
    fn xuannv_routing_addendum_contains_5_rules() {
        let txt = XUANNV_DISPATCH_ROUTING_ADDENDUM;
        // 防漂移：5 条规则核心关键词必须在文案里
        assert!(txt.contains("required_tags"));
        assert!(txt.contains("pinned_node"));
        assert!(txt.contains("local"));
        assert!(txt.contains("erp"));
        assert!(txt.contains("home"));
        assert!(txt.contains("mac-local"));
    }

    #[test]
    fn inject_xuannv_routing_cc_when_no_existing_addendum_sets_routing_only() {
        let mut cfg = CcLaunchConfig::default();
        assert!(cfg.append_system_prompt.is_none());
        inject_xuannv_routing_cc(&mut cfg);
        let got = cfg.append_system_prompt.expect("should be set");
        assert!(got.contains("required_tags"));
        assert!(got.contains("pinned_node"));
    }

    #[test]
    fn inject_xuannv_routing_cc_appends_after_existing_addendum() {
        // 玄女实战场景：sentinel skip xuannv，但她可能有别的 role-specific addendum；
        // routing 注入应跟在已有内容后面，不覆盖。
        let mut cfg = CcLaunchConfig {
            append_system_prompt: Some("# 你是玄女 ...".to_string()),
            ..Default::default()
        };
        inject_xuannv_routing_cc(&mut cfg);
        let got = cfg.append_system_prompt.expect("should be set");
        assert!(got.starts_with("# 你是玄女"), "role 段应在前");
        assert!(got.contains("required_tags"), "routing 段应被附加");
    }

    #[test]
    fn is_globally_disabled_reads_truthy_env() {
        // 注意：env 测试天然有进程级 race。这里用 unsafe set/unset，且
        // 用独特的 env var 名（FUXI_DISABLE_SENTINEL_ADDENDUM 本身）避免别的
        // 测试同时改它。本测必须串行——unfortunately cargo test 默认并行；
        // 改用一个临时 helper 比较稳：直接构造 env 变化窗口验证即可。
        // 简单起见用 std::env::set_var (unsafe in 1.80+) 包 unsafe block。
        unsafe {
            std::env::set_var("FUXI_DISABLE_SENTINEL_ADDENDUM", "1");
        }
        assert!(is_globally_disabled());
        unsafe {
            std::env::set_var("FUXI_DISABLE_SENTINEL_ADDENDUM", "true");
        }
        assert!(is_globally_disabled());
        unsafe {
            std::env::set_var("FUXI_DISABLE_SENTINEL_ADDENDUM", "0");
        }
        assert!(!is_globally_disabled());
        unsafe {
            std::env::remove_var("FUXI_DISABLE_SENTINEL_ADDENDUM");
        }
        assert!(!is_globally_disabled());
    }
}
