# 决策独立文档索引

每份文档一个决策 + 为什么这么决策 + 什么时候 override。AI 友好 = grep 精确定位，不拉大综述。

| # | 文件 | 一句话 |
|---|---|---|
| 01 | `01-agent-team-parallel.md` | 并行 agent team 用独立 cc 进程（不用 Task subagent） |
| 02 | `02-soul-first-skill.md` | Skill 是角色包（不止 SKILL.md）；body 首段必须是 soul |
| 03 | `03-tui-task-tree-override.md` | TUI 左栏任务树（不是 agent roster）—— override C2 初版 |
| 04 | `04-intervene-idle-degrade.md` | intervene Idle 门客自动退化 dispatch（反玄女推荐的 short-term） |
| 05 | `05-conversation-switch-keep-wire.md` | ~~让贤 wire 保留不删（反 T2 reviewer）；发起源延 v1.1~~ — **已被 08 override** |
| 06 | `06-cultural-naming-scheme.md` | 文化底蕴命名总表（策府/甲骨/点将台/更漏/铸牒司/让贤等） |
| 07 | `07-recall-scope.md` | P2 召回 = 整个工作环境（worktree + cli session）；通用 wire + cc 特化层 |
| 08 | `08-conversation-switch-removed.md` | 让贤（ConversationSwitch）拆除 · override 05 · M4.3 兑现 |
| 09 | `09-tui-opencode-learnings.md` | TUI 12 条系统性借鉴 opencode · M4-REDUX 一批全做 |
| 10 | `10-task-bound-agents.md` | Task-bound agent lifecycle + 任务树 UI + `#N` 命名 + `@` 消歧 popup |
| 11 | `11-tui-cc-learnings-v2.md` | TUI 借鉴 cc 第二轮 · 12 条分三批（Batch C/D/E）|
| 12 | `12-dist-worker-true-concurrency.md` | dist worker 真并发 + cancel / heartbeat / capacity 对账 |
| 13 | `13-deliverable-boundary-handoff.md` | 门客交付边界：完成后必须请求 review / handoff |
| 14 | `14-im-mobile-frontend.md` | IM mobile-first 前端作为主入口 |
| 15 | `15-im-task-tree-pager.md` | IM 任务树分页和可读性约束 |
| 16 | `16-im-tab-bar-task-thread.md` | IM tab bar + task thread 交互边界 |
| 17 | `17-im-deploy-decoupling.md` | IM 部署组合与 dist/controller 解耦边界 |
| 18 | `18-agentic-engineering-collaboration.md` | Agentic Engineering 协作原则：用户定目标边界，Codex 交付可验证闭环 |

## 写新 decision 怎么做

- 文件名 `<编号>-<短横线-slug>.md`，编号 ZeroPad 2 位（`07`/`08`/...）
- 结构（软约束，可增减）：
  ```
  # Decision NN · 短标题
  日期 / 状态
  ## 背景（引用触发它的对话 / commit / review）
  ## 决策
  ## 理由
  ## 代价
  ## 何时不适用 / 何时重审
  ## 参考
  ```
- 每份 ≤ 100 行；大了拆
- 有"反意见"的决策要写清**反了谁、为什么反**（decision 04/05 是范例）

## 何时改老 decision

**不改**老 decision。如要 override，**写新 decision N+1** 明说"override decision M"，两者共存。目的是保留决策轨迹，让下个 session 看得到"为什么从 X 变成 Y"。

反例（禁止）：直接编辑 `03-tui-task-tree-override.md` 把"任务树"改回"roster"—— 这抹掉了历史。
