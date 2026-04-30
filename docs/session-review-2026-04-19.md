# Session Review · 2026-04-19

> [!WARNING]
> `historical`：此文档为 2026-04-19 的过程记录，保留用于追溯当时决策，不再作为当前执行入口或状态口径。
> 当前状态以 `docs/status/now.md` 为准。

> 这份文档**不是给用户看的**，是给下一个 Claude Code 接手人看的**过程性记录**。
>
> 和 HANDOFF.md 的区别：HANDOFF 写"做了什么、下一步候选什么"（结论性）；这份写"为什么是这个方向、否决了什么、读了什么源、没想清楚什么"（过程性）。
>
> 下次 session 开局：**HANDOFF 是必读，这份是接手人真正需要读懂的**。
>
> 本文档由后续 session 的 Agent 扫描 04-19 session transcript 提炼得出（主 transcript：`~/.claude/projects/-Users-e0-7-fuxi/22b35ff0-9820-40de-be13-e49f551fcfe5.jsonl`）。

---

## 1. 昨晚借鉴 ComposioHQ 的具体路径

**审计对象**：`ComposioHQ/agent-orchestrator @ 509fb12`（PR #1300 merge 点）  
**曾 clone 到**：`/tmp/composio-audit/`（临时目录，凌晨 agent 完活后大概率已清理，需要可以重 clone）

### 精确读过的源码位置 ↔ fuxi 落点

| composio 文件 | 读的是什么 | fuxi 对应 / 姿态 |
|---|---|---|
| `packages/core/src/types.ts:391/456/601/644/718/994/1028` | Runtime / Agent / Workspace / Tracker / SCM / Notifier / Terminal **7 个插件槽的 trait 形状** | `crates/fuxi-core/` 的 `Agent` / `Runtime` / `Workspace` trait — 借 trait 形状，Rust 重写 |
| `packages/core/src/types.ts:1049-1086` + `:1089` | 30+ 种 `OrchestratorEvent` 的**事件词汇表** | `crates/fuxi-core/src/event.rs` 的 `EventKind` 标签联合 |
| `packages/core/src/plugin-registry.ts:38-68` + `docs/PLUGIN_SPEC.md` | plugin manifest + `{manifest, create, detect}` 注册契约 | 只偷思想，**未复刻**（fuxi 用 enum 而不是动态 registry） |
| `packages/plugins/agent-aider/src/index.ts`（325 LoC 单文件） | 适配器模板 | **`fuxi-agent-codex` 和 `fuxi-agent-cc` 的参考模板** |
| `packages/core/src/lifecycle-manager.ts:1537`（`notifyHuman`） + `packages/web/src/app/api/events/route.ts:17` | 发现 composio 的"实时"是 **5s HTTP 轮询**，无内部 event bus | **反向案例** — fuxi 的独创赌注 #1（真实时 Firehose）正是针对这个 |
| `packages/core/src/session-manager.ts:2019`（`send()`） | 发消息直接写目标 tmux pane，**不抄送 orchestrator** | **反向案例** — fuxi 的独创赌注 #2（InterventionProxy）动机 |
| `packages/core/src/prompts/orchestrator.md:9-13` | 明确声明 "orchestrator session must never own a PR / read-only" | **反向案例** — fuxi 的 ConversationSwitch（独创赌注 #3）刻意违反这条 |
| `packages/plugins/notifier-composio/src/index.ts:52-77` | 确认无 vendor lock-in 硬依赖 | 排除掉，不做 Composio SaaS 绑定 |

### 明确不取的

- composio 代码本体（语言隔离）
- Composio SaaS 绑定 / notifier-composio
- team-anya 侧：飞书多人通道、三层 MCP 工具、固定五角色
- 这些是**设计时就决定不拿**的，不是遗忘

---

## 2. 否决的替代方案（过程性决策链）

| 方案 | 结论 | Why not |
|---|---|---|
| **Path A — depend-and-extend ComposioHQ** | 否决 | 切到 Rust 直接废：不能 `npm install` TS 为依赖；且 `session-manager.ts` (2699 LoC) + `lifecycle-manager.ts` (1995 LoC) 处于 PR-1300 活跃重构，跟屁股不值 |
| **Path B — fork ComposioHQ** | 否决（备选保留） | 上游 churn 成本高，换 20% 收益不划算。作为"子转场事件颗粒度真·刚需时"的最后备选 |
| **Path D — 彻底无视 composio** | 否决 | 无依据：代码干净、MIT、无 vendor lock、设计智慧值得学 |
| **S2 pty-wrap CLI** | 否决 | 极脆弱，CLI 输出格式改就挂。改走 S6（各 CLI headless / JSON I/O） |
| **S1 SDK 驱动** | 否决 | 放弃 cc/codex 的 tool harness/slash command 现成能力 = 重造 mini-cc |
| **S3 底层 API + 自造工具** | 否决 | 同上，重造轮子 |
| **S4 MCP 协议化工具层** | 否决 | 用户 explicit："不咋喜欢 mcp 了 / CLI 才是 AI 原生"；MCP 是"agent 调工具"层，A2A 才是"agent↔agent"层，两层分开 |
| **NATS / Redis 作事件总线** | 否决 | 单机场景 tokio broadcast + SQLite WAL 够用；P3 跨机时再谈 |
| **TypeScript 技术栈** | 否决 | 和 anya 同语言是优势，但 Rust 给毕设 contribution 更强 + 天然和 composio 代码隔离 |
| **claude-squad 借代码** | 否决 | AGPL-3.0 许可"毒"，只学 UX 不抄代码 |
| **Firehose 做到工具调用级颗粒度** | 延后（v1 只做转场级） | 避免一开始就 fork composio core；工具调用级留到未来 |

---

## 3. 遗留未决 / 没想清楚的地方

### 设计文档 §11 留的未决

- **毕设 deadline 具体日期** — 04-19 session 用户回复"时间还够"，未给日期。**04-19 下午用户明确：毕设不是 ddl，最终蓝图才是，这条已过时**
- **玄女 / 门客 profile / prompt 内容** — 04-19 下午决策：采纳 [Anthropic Agent Skills](https://github.com/agentskills/agentskills) 格式，`~/.fuxi/skills/<role>/SKILL.md`
- **云 burst 触发策略**（手动 / 自动）— M3+
- **Firehose 颗粒度长期策略** — v1 转场级；是否 fork composio core 做工具调用级待长期评估

### orchestrator API 形状未定

- `dispatch_to_any` 的 "role → profile 工厂" 在 reviewer 报告 L1017 被点出 **API 形态待定**（方案 A vs 方案 B），未下结论。当前代码能工作但不一定是最终 API
- **下次触碰 `fuxi-orchestrator::dispatch_to_any` 前回去读 L1017 报告**

### reviewer 指出的 follow-up（只修了 S1/S2/S3，剩下未修）

- transcript L1017 报告后半段列出其它 follow-up，未提炼到 ADR / HANDOFF
- **下一个碰 orchestrator 的接手人需要回去看这份报告**，地址是 session transcript L1017
- TODO：将 L1017 后半段 follow-up 正式提炼成 issue / task 列表

---

## 4. "companion" 项目线索（supervisor 未定位）

- **结论：未定位**
- transcript 搜 "companion"：只在 brainstorm CLI skill 的 "Offer visual companion" 文案里出现，**不是项目名**
- anya 代码里的 `companion` 是其作者的私人参照（可能是 Anthropic 内部 SDK reference、`@anthropic-ai/claude-agent-sdk` 打包前源、或 `anthropics/claude-agent-sdk-demos`），公开源无匹配
- **实操建议**：直接读 `/Users/e0_7/team-anya/apps/server/src/broker/backends/claude-code-backend.ts` (1066 行)，已是 companion 的完整二次呈现版。不必找原源

---

## 5. 已踩过的坑（CLAUDE.md 未写的）

### macOS tempdir symlink（L816）
- `TempDir::new()` 返回 `/var/folders/...`
- `git worktree list --porcelain` 报 `/private/var/folders/...`
- **列表比对两侧都要 `canonicalize()`**，否则 `list()` 匹配失败

### Codex model fallback（L1004）
- `DEFAULT_MODEL_FALLBACK = "gpt-5.1-mini"` 在 **ChatGPT-account auth 下被拒** `invalid_request_error`
- E2E 测试姿势：用空串让 codex 自选；API key 用户需 `FUXI_CODEX_MODEL` 覆盖

### Codex exec 不支持 follow-up（L1004）
- `send_message` 直接返回 `CoreError::Other`
- **spawn-per-dispatch 是唯一姿势**（不像 cc 支持 stream 续写）
- 未来如果要 codex 也做 "single worker 多轮对话" 需要换成 codex 的 `conversation` 模式（另一套 API）

### reviewer 修掉的 bug（记忆以防新适配器复现）
- **S1 · Agent ID 双生**：`spawn_worker` 预生成 id 和 `CcAgent::launch` 内部 `AgentId::new()` 不一致，导致 `AgentSpawning`/`AgentReady` 在事件流上属于两个 id。**修复：所有适配器提供 `launch_with_id`**
- **S2 · pump Blocked 误判**：`TaskState::Blocked` 允许 `→ Ready`，原版把 Blocked 当 terminal → shelf 永久 Busy。**修复：pump 判断 Blocked 不是终态**
- **S3 · `dispatch_to_any` TOCTOU**：find-then-set-status 两步非原子，并发会双派。**修复：原子 `claim_*_by_*` 形态**

已在 commit `360a31e` 修掉，但**下一个新适配器（gemini/opencode）必须回头看这三条**，否则会复现。

---

## 6. 结论文档没写的 "因为 X 所以 Y"

### 为何 Rust 而不是 TS
- 毕设叙事角度 greenfield Rust >> TS 插件（读论文 related work / baseline 的 novel contribution 更强）
- ComposioHQ 作论文 baseline；fuxi 的三独创（ConversationSwitch / InterventionProxy / 真实时 Firehose）是明确 novel
- Rust 天然和 composio TS 代码隔离，防止"无脑抄"

### 为何自实现 A2A v1.0
- Rust 生态**无 A2A SDK**
- Python / Go 有 v1.0，TS 只有 0.3
- "自实现 = 必要 + 毕设 contribution"（ADR-002 立论基础）

### 为何有 ConversationSwitch
- 04-19 凌晨用户用"开发 IM 应用"场景推演
- Claude 意识到"员工 agent 也会直接跟人对话，不只是顶层 agent"
- 这直接催生了主对话权转交的独创设计
- composio 的 "orchestrator must be read-only" 是反面教材

### 为何有 InterventionProxy
- composio `session-manager.ts:2019` 的 `sessionManager.send` 直接写目标 pane，**orchestrator 不知道**
- fuxi 刻意违反这一点 — 玄女有知情权（抄送）无否决权（不能阻止）
- 这是"玄女世界模型永远和现实一致"的技术担保

### 为何用户偏好（L147）
- 选型（TS vs Go、SQLite vs NATS、TUI vs Web）**全权交给 Claude**
- 用户只在"宏观链路有问题时叫停"
- **这解释了为什么 HANDOFF §4 的"原话"是"工程性的决策你不该问我"**

### P2 基建被 P1 demo 吃上（commit `bbaf2b2`）
- 下一任 session 看到的 `fuxi demo` 是 P2 orchestrator 驱动的，**不是原始 P1 的 cc 直连**
- 这个切换很突然，但是**合理**：P1 demo 不再是特殊路径，就是 orchestrator 的最简用例

---

## 7. 对 HANDOFF / CLAUDE.md 的更新建议

本次 session（04-19 下午）产生以下**需要同步进永久文档**的信息：

- [x] `docs/references.md` — 已建（本次 session）
- [x] `docs/superpowers/specs/2026-04-19-v0.1-scenario.md` — 已建（本次 session）
- [ ] `CLAUDE.md` 常见陷阱 — 加入本文 §5 的 3 条 macOS/Codex/reviewer 教训
- [ ] `HANDOFF.md` §11 "不要做的事" — 加入 "不要 dispatch 到未提供 `launch_with_id` 的适配器"（S1 教训升级为铁律）
- [ ] 设计文档 §11 "未决事项" — 移除"毕设 deadline"；更新"profile 格式"为 agentskills

主 session 会在完成 v0.1 spec 后批量处理。

---

## 8. 本次 transcript 扫描来源

- `/Users/e0_7/.claude/projects/-Users-e0-7-fuxi/22b35ff0-9820-40de-be13-e49f551fcfe5.jsonl`（1180 行，3.4 MB，主干）
- `/Users/e0_7/.claude/projects/-Users-e0-7-fuxi/842959fc-61b6-4d5f-a200-5ab398736077.jsonl`（10 行，29 KB，短）
- `/Users/e0_7/.claude/projects/-Users-e0-7-fuxi/e6215007-d153-4aae-b7e6-9d0c1377c362.jsonl`（本次 session 自身的 transcript 部分）

注：`-Users-e0-7-xihe/` 是同一目录的 symlink，无需重复扫。
