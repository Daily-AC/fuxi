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
use fuxi_memory::{HetuStore, UserProfileStore};

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

## 文件级交付（Decision 22）

如果你产出了**用户要拿走的文件**（markdown 报告 / csv / 配置 / 图等，**不**包括代码 commit），
在发 sentinel **之前**先在 Bash 里把文件落进 PWA 收件箱：

```bash
fuxi deliverable produce --project <slug> --task <task-uuid> --kind <5 类之一> file1 [file2 ...]
```

`<task-uuid>` 形态 `task-<uuid>`——若派活 prompt 里有标注用它（同 task 的多份
deliverable 集中在一个 bucket，便于 PWA 按 task 聚合）；不确定就**省略 `--task`**，
平台会随机生成（PWA 仍能看到，只是不跟此 task 关联）。`<slug>` 是用户已注册的 project；
不知道就发 `decision_request` sentinel 问玄女。

平台会 sha256 + 复制到 `~/.fuxi/projects/<slug>/deliverables/<task>/` + 发 DeliverableProduced
事件 + PWA「交付」tab 立刻可见可下载。

**纯代码 commit 不要走 produce**——commit 已是 git artifact，PWA 后续会接 PR 视图。
**纯 summary 文字不需要 produce**——sentinel summary 已够玄女决策。

## 轻量 inline 推送（fuxi note）

产出**不是文件级工件**、是一段稍长的 markdown 或纯文本（小报告 / 摘要 / 调研片段，≤ 256KB），
既不必让用户落地下载，也比 sentinel summary 一两句长——用 `fuxi note` 直贴到任务对话流：

```bash
fuxi note --task <task-uuid> [--from <agent-uuid>] [--mime text/markdown|text/plain] <file>
```

PWA 任务私聊页渲染为左侧 worker 卡片（`text/markdown` 走 md 渲染，`text/plain` 走 `<pre>`）。
`<task-uuid>` 同 deliverable produce 用法（派活 prompt 里有标注）；`--from` 不传走 events.db
反查 `TaskDispatched.to`（单门客单 task 场景准；fan-out / re-dispatch 场景须显式传）。

**三档心智模型**（按工件大小 + 是否落地）：
- 一两句 → sentinel `summary` 字段（不调 note，也不调 produce）
- 中段 markdown / 摘要、不要用户下载 → `fuxi note`（贴对话流）
- 用户要下载的工件、二进制、> 256KB → `fuxi deliverable produce`（落收件箱）

**note 不替代 sentinel**——note 只贴内容到对话流，玄女注意力仍靠 sentinel JSON 拍醒。
完整流程：先 `fuxi note` 把 markdown 贴上去 → 再发 sentinel 让玄女知道有东西可看
（sentinel 的 `artifact_ref` 可填 `note:<filename>` 提示她去对话流看）。
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

// ──────────────────────────────────────────────────────────────────
// memory-v2（论文 arXiv:2604.14004 Memory Transfer Learning）注入桥
//
// 三表心智：
//   - oracle_facts   = trajectory 层 → **绝不**注入门客 prompt（negative transfer）
//   - user_profile   = summary 层    → 注入「用户身份卡」段
//   - hetu_patterns  = insight 层    → 注入「历史心法」段（至多 5 条，按抽象度 + 时间排序）
//
// 黑名单：xuannv（接收方/对话上下文已含用户原话，再注入会污染）/ extractor（幕后工
// 不需要身份卡）/ cangjie（自吞循环——它就是写 hetu 的人，再读自己写的内容会放大噪声）。
// 跟 sentinel 黑名单概念同族但语义不同（sentinel 黑名单是"不教 sentinel"，memory 黑
// 名单是"不注入身份卡 + 心法"），故各持各的列表。
// ──────────────────────────────────────────────────────────────────

/// memory-v2 注入条目数上限——多 > 5 条会稀释门客 prompt（论文：注入越多越易过拟合）。
const MEMORY_MAX_INSIGHTS: usize = 5;

/// 是否给该 role 注入 memory（user_profile + hetu_patterns）。
///
/// 黑名单：
/// - **xuannv**：玄女自身就是用户对话的接收方，身份卡跟对话上下文重复
/// - **extractor**：fact 提取器，幕后跑，不属"门客在用户领域执行任务"模型
/// - **cangjie**：仓颉是 insight 提取器；让它读自己写的 hetu 形成自吞循环，放大噪声
pub fn should_inject_memory_for_role(role: &str) -> bool {
    !matches!(role, "xuannv" | "extractor" | "cangjie")
}

/// 拼装 memory addendum 文案——纯字符串组装，无 IO。
///
/// 入参：
/// - `summary` 来自 `UserProfileStore::summary()`（≤ 200 字凝练身份卡，可能空串）
/// - `insights` 是若干条 `(pattern, abstraction_score)`——已按抽象度+时间降序，
///   函数内不再排序，仅截前 [`MEMORY_MAX_INSIGHTS`] 条
///
/// 文案契约（玄女 instructions / 论文逻辑都依赖这两个 heading 字面量）：
/// - 用户身份卡段标题：`## 用户身份卡（必读）`
/// - 历史心法段标题：`## 你这个角色（{role}）的历史心法（论文：抽象度决定可迁移性）`
///
/// 空 store 走"暂无"占位——让门客显式知道"这是 memory-v2 注入位但当前空"，
/// 比省略整段更不歧义（防止它误认为 system prompt 出错）。
fn render_memory_addendum(role: &str, summary: &str, insights: &[String]) -> String {
    let mut out = String::from("---\n## 用户身份卡（必读）\n");
    if summary.trim().is_empty() {
        out.push_str("（暂无——用户画像还没记，等玄女通过 `fuxi profile set` 入卡）\n");
    } else {
        out.push_str(summary.trim());
        out.push('\n');
    }
    out.push_str("\n## 你这个角色（");
    out.push_str(role);
    out.push_str("）的历史心法（论文：抽象度决定可迁移性）\n");
    if insights.is_empty() {
        out.push_str("（暂无——仓颉还没积累到这个角色的可复用心法）\n");
    } else {
        for ins in insights.iter().take(MEMORY_MAX_INSIGHTS) {
            // 心法可能跨多行；统一压成单行 markdown bullet（首行加 `- `，剩余行
            // 撒 leading 空格保持 list item 连续性）。多行心法在论文里少见但也别让
            // 它撑破 markdown 结构。
            let mut lines = ins.lines();
            if let Some(first) = lines.next() {
                out.push_str("- ");
                out.push_str(first);
                out.push('\n');
                for rest in lines {
                    out.push_str("  ");
                    out.push_str(rest);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// 拉 user_profile + hetu_patterns 心法，组装注入文案——cc + codex 共用一段。
async fn fetch_and_render(
    role: &str,
    profile: &UserProfileStore,
    hetu: &HetuStore,
) -> Result<String, fuxi_memory::Error> {
    let summary = profile.summary().await?;
    let recent = hetu.recent_for_role(role, MEMORY_MAX_INSIGHTS).await?;
    let insights: Vec<String> = recent.into_iter().map(|p| p.pattern).collect();
    Ok(render_memory_addendum(role, &summary, &insights))
}

/// cc 路径 memory 注入——拉 store → 拼 addendum → 拼到 `cc_cfg.append_system_prompt`。
///
/// 黑名单 role / 全局 disable → noop（store 都不读，省 IO）。
/// 已有 addendum 时**追加**（保留 sentinel + role-specific 在前）。
pub async fn inject_role_memory_cc(
    cfg: &mut CcLaunchConfig,
    role: &str,
    profile: &UserProfileStore,
    hetu: &HetuStore,
) -> Result<(), fuxi_memory::Error> {
    if is_globally_disabled() || !should_inject_memory_for_role(role) {
        return Ok(());
    }
    let addendum = fetch_and_render(role, profile, hetu).await?;
    let trimmed = addendum.trim_end();
    cfg.append_system_prompt = match cfg.append_system_prompt.take() {
        Some(existing) if !existing.trim().is_empty() => Some(format!("{existing}\n\n{trimmed}")),
        _ => Some(trimmed.to_string()),
    };
    Ok(())
}

/// codex 路径 memory 注入——同 cc 思路，但落在 `profile.system_prompt` 末尾
/// （codex `compose_prompt` 把整段 prepend 到 dispatch prompt）。
pub async fn inject_role_memory_codex(
    profile_obj: &mut AgentProfile,
    role: &str,
    profile: &UserProfileStore,
    hetu: &HetuStore,
) -> Result<(), fuxi_memory::Error> {
    if is_globally_disabled() || !should_inject_memory_for_role(role) {
        return Ok(());
    }
    let addendum = fetch_and_render(role, profile, hetu).await?;
    let trimmed = addendum.trim_end();
    profile_obj.system_prompt = if profile_obj.system_prompt.trim().is_empty() {
        trimmed.to_string()
    } else {
        format!("{}\n\n{}", profile_obj.system_prompt, trimmed)
    };
    Ok(())
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
    fn addendum_text_contains_fuxi_note_teaching() {
        // P2.7 新增 inline 推送章节防漂移
        let txt = SENTINEL_ADDENDUM_TEXT;
        assert!(txt.contains("fuxi note"), "应教 fuxi note 用法");
        assert!(txt.contains("256KB"), "应说明 size 上限");
        assert!(txt.contains("text/markdown"), "应说明 mime 选项");
        assert!(
            txt.contains("不替代 sentinel"),
            "应强调 note 不替代 sentinel"
        );
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

    // ── memory-v2 注入桥测试 ──

    #[test]
    fn should_inject_memory_skips_xuannv_extractor_cangjie() {
        assert!(!should_inject_memory_for_role("xuannv"));
        assert!(!should_inject_memory_for_role("extractor"));
        assert!(!should_inject_memory_for_role("cangjie"));
        // 其他 role 注入
        assert!(should_inject_memory_for_role("luban"));
        assert!(should_inject_memory_for_role("luban-codex"));
        assert!(should_inject_memory_for_role("zhudiesi"));
    }

    #[test]
    fn render_memory_addendum_with_summary_and_insights() {
        let txt = render_memory_addendum(
            "luban",
            "identity: 以琳；tone: 直球",
            &["TDD 红绿循环".to_string(), "Cargo.lock rm 重建".to_string()],
        );
        assert!(txt.contains("## 用户身份卡（必读）"));
        assert!(txt.contains("identity: 以琳；tone: 直球"));
        assert!(txt.contains("## 你这个角色（luban）的历史心法"));
        assert!(txt.contains("- TDD 红绿循环"));
        assert!(txt.contains("- Cargo.lock rm 重建"));
        assert!(txt.contains("抽象度决定可迁移性"));
    }

    #[test]
    fn render_memory_addendum_caps_at_5_insights() {
        let many: Vec<String> = (0..10).map(|i| format!("心法 {i}")).collect();
        let txt = render_memory_addendum("luban", "identity: x", &many);
        // 前 5 条入，后 5 条不入
        assert!(txt.contains("心法 0"));
        assert!(txt.contains("心法 4"));
        assert!(!txt.contains("心法 5"));
        assert!(!txt.contains("心法 9"));
    }

    #[test]
    fn render_memory_addendum_empty_uses_placeholders() {
        let txt = render_memory_addendum("luban", "", &[]);
        // 空身份卡 → 占位
        assert!(txt.contains("（暂无——用户画像还没记"));
        // 空心法 → 占位
        assert!(txt.contains("（暂无——仓颉还没积累"));
        // 标题段仍在（玄女 instructions / 测试都靠 heading 字面）
        assert!(txt.contains("## 用户身份卡"));
        assert!(txt.contains("## 你这个角色（luban）的历史心法"));
    }

    /// memory addendum 不能含 oracle_facts 任何字段——论文核心 safety boundary。
    #[test]
    fn render_memory_addendum_does_not_leak_oracle_keywords() {
        let txt = render_memory_addendum("luban", "identity: x", &["foo".to_string()]);
        // 几个 oracle_facts 内部字段名——render 不应该提到
        assert!(!txt.contains("subject"));
        assert!(!txt.contains("predicate"));
        assert!(!txt.contains("object"));
        assert!(!txt.contains("oracle_facts"));
    }

    #[tokio::test]
    async fn inject_role_memory_cc_appends_after_existing() {
        use fuxi_memory::{HetuStore, NewPattern, NewProfile, UserProfileStore};
        let profile = UserProfileStore::connect_memory().await.unwrap();
        profile
            .record(NewProfile::new("identity", "以琳，工程师"))
            .await
            .unwrap();
        let hetu = HetuStore::connect_memory().await.unwrap();
        hetu.record(NewPattern::insight("luban", "TDD 红绿").with_abstraction_score(0.9))
            .await
            .unwrap();

        let mut cfg = CcLaunchConfig {
            append_system_prompt: Some("# 你是鲁班".into()),
            ..Default::default()
        };
        inject_role_memory_cc(&mut cfg, "luban", &profile, &hetu)
            .await
            .unwrap();
        let got = cfg.append_system_prompt.unwrap();
        // role-specific 在前，memory 在后
        assert!(got.starts_with("# 你是鲁班"));
        let role_pos = got.find("# 你是鲁班").unwrap();
        let card_pos = got.find("用户身份卡").unwrap();
        assert!(role_pos < card_pos);
        // 内容齐全
        assert!(got.contains("以琳"));
        assert!(got.contains("- TDD 红绿"));
    }

    #[tokio::test]
    async fn inject_role_memory_cc_blacklist_noop() {
        use fuxi_memory::{HetuStore, NewPattern, NewProfile, UserProfileStore};
        let profile = UserProfileStore::connect_memory().await.unwrap();
        profile
            .record(NewProfile::new("identity", "x"))
            .await
            .unwrap();
        let hetu = HetuStore::connect_memory().await.unwrap();
        hetu.record(NewPattern::insight("xuannv", "y"))
            .await
            .unwrap();

        for role in ["xuannv", "extractor", "cangjie"] {
            let mut cfg = CcLaunchConfig::default();
            inject_role_memory_cc(&mut cfg, role, &profile, &hetu)
                .await
                .unwrap();
            assert!(
                cfg.append_system_prompt.is_none(),
                "{role} 应被黑名单 noop，结果：{:?}",
                cfg.append_system_prompt
            );
        }
    }

    #[tokio::test]
    async fn inject_role_memory_cc_empty_stores_uses_placeholders() {
        use fuxi_memory::{HetuStore, UserProfileStore};
        let profile = UserProfileStore::connect_memory().await.unwrap();
        let hetu = HetuStore::connect_memory().await.unwrap();
        let mut cfg = CcLaunchConfig::default();
        inject_role_memory_cc(&mut cfg, "luban", &profile, &hetu)
            .await
            .unwrap();
        let got = cfg.append_system_prompt.expect("should set");
        assert!(got.contains("用户身份卡"));
        assert!(got.contains("（暂无"));
    }

    #[tokio::test]
    async fn inject_role_memory_codex_appends_to_system_prompt() {
        use fuxi_memory::{HetuStore, NewPattern, NewProfile, UserProfileStore};
        let profile = UserProfileStore::connect_memory().await.unwrap();
        profile
            .record(NewProfile::new("tone", "直球"))
            .await
            .unwrap();
        let hetu = HetuStore::connect_memory().await.unwrap();
        hetu.record(NewPattern::insight("luban-codex", "rule X"))
            .await
            .unwrap();

        let mut p = make_profile("luban-codex", "# 你是鲁班 codex");
        inject_role_memory_codex(&mut p, "luban-codex", &profile, &hetu)
            .await
            .unwrap();
        assert!(p.system_prompt.contains("# 你是鲁班 codex"));
        assert!(p.system_prompt.contains("用户身份卡"));
        assert!(p.system_prompt.contains("直球"));
        assert!(p.system_prompt.contains("- rule X"));
    }

    #[tokio::test]
    async fn inject_role_memory_codex_blacklist_noop() {
        use fuxi_memory::{HetuStore, UserProfileStore};
        let profile = UserProfileStore::connect_memory().await.unwrap();
        let hetu = HetuStore::connect_memory().await.unwrap();
        let mut p = make_profile("cangjie", "# 仓颉本体");
        inject_role_memory_codex(&mut p, "cangjie", &profile, &hetu)
            .await
            .unwrap();
        assert_eq!(p.system_prompt, "# 仓颉本体");
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
