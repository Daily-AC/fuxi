# 伏羲招贤机制调研 —— skill 管理方案对比与落地设计

> 场景：玄女遇到现有门客 role 覆盖不了的任务时，动态生成一份 agentskills 格式的 skill 包，落到 `~/.fuxi/skills/<new-role>/`，下次 spawn 门客时自然 load。本文先对比业界方案，再给出伏羲招贤机制的落地建议。

## 1. 成熟方案一句话定性

| 方案 | 定性 |
|---|---|
| **anthropics/skills** | 官方 skill 仓库，`skills/<name>/SKILL.md` + 可选 `scripts/` `references/` `assets/`；frontmatter 只强制 `name` / `description`，`allowed-tools` 空格分隔且**只有 Claude Code CLI 尊重**、SDK 忽略。 |
| **Claude Code plugin marketplace** | 面向"多 skill + command + MCP"的打包分发层；`claude plugin marketplace add <repo>` + `claude plugin install <name>@<market>`，`.claude-plugin/marketplace.json` 作 manifest；非交互子命令可脚本化。 |
| **agentskills.io spec** | 跨厂商标准化尝试：规定目录约束（`name` ≤ 64 小写+连字符、`description` ≤ 1024、可选 `license` / `compatibility` / `metadata`），`scripts/ references/ assets/` 的职责分离。 |
| **VS Code agent skills** | 工作区优先：`.github/skills/` > `.claude/skills/`（同名冲突时前者胜），**不支持 user-global skill dir**，强制走 workspace。 |
| **block/goose recipes** | YAML 配方而非 markdown skill；`~/.config/goose/recipes/` 全局库 + `GOOSE_RECIPE_PATH` 多路径 + `GOOSE_RECIPE_GITHUB_REPO` 远程拉取；没有 version pin，靠 git ref。 |
| **sst/opencode agents** | `~/.config/opencode/agents/` 或 `.opencode/agents/`，文件名即 agent 名，frontmatter 是元数据、body 是 system prompt；`AGENTS.md` 作项目级规则。 |
| **cursor rules** | `.cursor/rules/<rule>/RULE.md`（2.2 起），Team → Project → User 三级优先级，早者覆盖晚者；agent 自己生成 MDC 时**会丢 frontmatter**是已知坑。 |
| **OpenAI Agent Skills (Responses API)** | 云端版本化 skill bundle，`purpose=assistants` 文件上传，支持 version pin；Assistants API 2026-08 下线，后继是 Responses API。 |
| **Letta persona/memory blocks + MemFS** | 不是 skill 文件而是 agent 内置的可变 persona 块；0.15 起用 git 给 memory 做版本 + rollback；模板版本化靠 ADE "Save new template version"。 |

## 2. 对比矩阵

| 方案 | 主安装路径 | 版本管理 | Marketplace | 动态生成 | 冲突策略 |
|---|---|---|---|---|---|
| anthropics/skills | 仓库内 `skills/` | git ref | 无（裸仓） | skill-creator 插件（Create/Eval/Improve/Benchmark 四模式） | 无，约定同名覆盖 |
| CC plugin marketplace | `~/.claude/plugins/` | `@ref` 锚 branch/tag | 有（`marketplace.json`） | 间接（装 skill-creator plugin） | marketplace 命名空间隔离 |
| agentskills.io | spec 不规定 | spec 不规定 | 无 | spec 不规定 | 靠实现 |
| VS Code | `.github/skills/` 或 `.claude/skills/`（workspace） | 无原生 | 无 | 有 UI 入口 | `.github/skills/` > `.claude/skills/` |
| Goose | `~/.config/goose/recipes/` | `GOOSE_RECIPE_GITHUB_REPO` | 无 | 无 | 路径顺序 |
| opencode | `~/.config/opencode/agents/` 或 `.opencode/agents/` | 无 | 无 | 手写 markdown | 项目覆盖全局 |
| cursor | `.cursor/rules/` | git | 无 | agent 自写（易丢 FM） | Team > Project > User |
| OpenAI skills | 云端 bundle | 强（version pin） | 无公开 | API 上传 | 由 assistant_id 绑定 |
| Letta | 内嵌 agent 状态 | git（MemFS）/ template version | 无 | 运行时 bash 工具改 | 按 block_label |

## 3. 伏羲招贤机制推荐方案

### 3.1 触发与决策

玄女每次拿到任务先做 **role match**：把任务描述 + 现有 `skills/*/SKILL.md` 的 `description` 一起喂给 matcher（先简单：向量 cosine + 阈值；v0.2 再上 LLM 评分）。无命中 → **招贤**。

招贤流程 3 段：

1. **起草（draft）**：玄女把任务意图 + 缺失能力摘要送一个独立 cc session（role=`skill-smith`），套 `skill-creator` 的 Create 模式产出 SKILL.md 草稿 + 可选 `references/`。**不让玄女自己写**——玄女是调度脑，skill 结构是专门知识。
2. **审核（review）**：默认 **user approval** 模式。草稿落到 `~/.fuxi/skills/.staging/<slug>/`，发 `SkillDraftProposed` 事件给 firehose，用户在 TUI 按 `y` 批准。纯本地毕设场景下也可以配 `fuxi.toml` 开 `skill.auto_approve = true` 跳过（有 `auto_approve_roles` 白名单）。
3. **落地（enroll）**：批准后 atomic rename 到 `~/.fuxi/skills/<slug>/`，写一条 `SkillEnrolled` 进事件流 + 写 `~/.fuxi/ledger/virtuous.jsonl`（贤士录，append-only，含 draft/approve/用过几次）。

### 3.2 生成方式选型

三选一混合：
- **主路：LLM 从 template 套**。仓库自带 `templates/skill-smith/{research,executor,reviewer}.tmpl.md`，玄女判断 archetype 传参给 skill-smith，填 `{name} {description} {allowed-tools} {body}`。保证 frontmatter 字段齐全、不丢格式（cursor 的坑规避）。
- **辅路：社区搜**。联 agentskills.io / anthropics/skills 做同义词搜索，如果高分命中就直接 fetch + 本地改名。MVP 可跳。
- **禁路：纯 LLM 自由创作**。实测 agent 写 SKILL.md 经常漏 frontmatter / 超 1024 字 / name 含大写，浪费 token 还要二次修。

### 3.3 安装位置：全局优先 + 项目可覆盖

沿用伏羲现有 `skill_loader::skills_root()` 的查找顺序（`$FUXI_SKILLS_DIR` → git-root/skills → cwd/skills → `$HOME/.fuxi/skills`），但招贤**默认写 `$HOME/.fuxi/skills/`**——跨项目复用是招贤的核心价值。项目想覆盖，把同 slug skill 放 `<project>/skills/` 就是。冲突规则与现有 loader 对齐：**查找顺序早的赢**（参考 VS Code 和 cursor 的"近处胜远处"思路）。

### 3.4 版本与冲突

- slug 不可变，内容迭代靠 `metadata.version: vN`（语义化 semver 太重，整数足够）。
- 升级策略：skill-smith 改进时写 `~/.fuxi/skills/<slug>/` **原地覆盖**，旧版本 `.bak-<unix_ts>/` 留档，30 天后自动清。不做 rollback UI（毕设 scope 外）。
- 并发招贤同名冲突：skill-smith 出 slug 时先查 `~/.fuxi/skills/` + `.staging/`，重名加 `-<rand4>`。
- 同 role 多版本不做——"玉牒唯一"比版本分裂靠谱。

### 3.5 用户审核：默认需要

毕设演示点就在"玄女会自己招贤"——**不能静默**。方案：
- TUI firehose 看到 `SkillDraftProposed` 高亮，显示 diff 预览 + `a`pprove / `r`eject / `e`dit。
- 未批准前 task **阻塞**（走已有 `task_blocked` 机制，reason=`pending_skill_approval`），批准后 `task_resumed`。
- `auto_approve = true` 给自己演示用；演示给导师时关掉更有说服力。

## 4. 命名建议

| 概念 | 术语 | 理由 |
|---|---|---|
| 招贤动作总称 | **招贤（zhaoxian）** | 已定 |
| 候选 role 目录 | **点将台** `~/.fuxi/skills/` | 点将=选派，语义对 role 库 |
| 单个 SKILL.md | **玉牒** | 古代皇家名册单页，对应 role 定义 |
| 草稿暂存区 | **榜文** `~/.fuxi/skills/.staging/` | 张榜待审，尚未入册 |
| 已招贤 log | **贤士录** `~/.fuxi/ledger/virtuous.jsonl` | 入册名录 |
| skill-smith agent role | **铸牒司** | 专职刻玉牒的门客 |
| 匹配失败告警事件 | `NoRoleMatched` → **求贤令** | 玄女向系统发求贤令 |

事件类型落到 `EventKind`：`SkillDraftProposed` / `SkillDraftApproved` / `SkillDraftRejected` / `SkillEnrolled` / `SkillSuperseded`。记得按宪法——加变体同步更 Firehose 渲染和 EventStore 持久化测试。

## 5. 与现有基建衔接

当前 `crates/fuxi-cli/src/skill_loader.rs` 只覆盖"读取 + 解析 frontmatter"最简路径。要支撑招贤，至少补：

1. **位置重构**：`skill_loader` 从 cli crate 挪到 `fuxi-core` 或独立 `fuxi-skills` crate——orchestrator、cli、未来的 skill-smith worker 都要读。
2. **frontmatter 升级**：现在是 `key: value` 平面字符串，招贤要写 `metadata.version` / `metadata.generated_at` / `metadata.generated_by` / `metadata.approved_by`——换 `serde_yaml`（或手写嵌套 KV，工作量差不多）。
3. **写侧 API**：`enroll(staged_path, slug) -> Result<()>`，atomic rename + 写贤士录；`stage(draft) -> staging_path`。
4. **load 扩展**：返回 scripts/ resources/ 的绝对路径清单（目前只用 body），门客启动时 mount 这些目录进 workspace。
5. **编辑器集成**：玄女收到 `NoRoleMatched` 时派 skill-smith（独立 Agent Adapter，和 cc/codex 同级），走 A2A 协议汇报草稿，而非 in-proc 函数调——保符合宪法 #1 "Headless agent 不显式沟通=没做"。
6. **Orchestrator 侧**：task 在 `pending_skill_approval` 时挂 `task_blocked`，复用薄片 F 已有机制，不新造。

---

**一句话总结**：抄 anthropics/skills 目录格式 + agentskills.io frontmatter 规范 + CC plugin 的"marketplace 就是 git repo 里一份 json"思想；招贤本质是**玄女派铸牒司门客，输出落榜文、用户审核入册点将台**——用伏羲已有的 A2A + EventBus + 门客机制实现，不引入新运行时。

---

## Sources

- [anthropics/skills](https://github.com/anthropics/skills)
- [skill-creator SKILL.md](https://github.com/anthropics/skills/blob/main/skills/skill-creator/SKILL.md)
- [agentskills.io specification](https://agentskills.io/specification)
- [Claude Code plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces)
- [anthropics/claude-plugins-official marketplace.json](https://github.com/anthropics/claude-plugins-official/blob/main/.claude-plugin/marketplace.json)
- [VS Code Agent Skills](https://code.visualstudio.com/docs/copilot/customization/agent-skills)
- [block/goose recipes — Saving Recipes](https://block.github.io/goose/docs/guides/recipes/storing-recipes/)
- [OpenCode Agents](https://opencode.ai/docs/agents/)
- [Cursor Rules MDC reference](https://github.com/sanjeed5/awesome-cursor-rules-mdc/blob/main/cursor-rules-reference.md)
- [Letta Memory blocks](https://docs.letta.com/guides/agents/memory-blocks)
- [Letta template versioning](https://docs.letta.com/guides/templates/versioning/)
- [Skills in OpenAI API (cookbook)](https://developers.openai.com/cookbook/examples/skills_in_api)
- [Claude Code CLI vs SDK allowed-tools inconsistency](https://github.com/anthropics/claude-code/issues/18737)
