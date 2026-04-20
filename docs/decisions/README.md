# 决策独立文档索引

每份文档一个决策 + 为什么这么决策 + 什么时候 override。AI 友好 = grep 精确定位，不拉大综述。

| # | 文件 | 一句话 |
|---|---|---|
| 01 | `01-agent-team-parallel.md` | 并行 agent team 用独立 cc 进程（不用 Task subagent） |
| 02 | `02-soul-first-skill.md` | Skill 是角色包（不止 SKILL.md）；body 首段必须是 soul |
| 03 | `03-tui-task-tree-override.md` | TUI 左栏任务树（不是 agent roster）—— override C2 初版 |
| 04 | `04-intervene-idle-degrade.md` | intervene Idle 门客自动退化 dispatch（反玄女推荐的 short-term） |
| 05 | `05-conversation-switch-keep-wire.md` | 让贤 wire 保留不删（反 T2 reviewer）；发起源延 v1.1 |
| 06 | `06-cultural-naming-scheme.md` | 文化底蕴命名总表（策府/甲骨/点将台/更漏/铸牒司/让贤等） |

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
