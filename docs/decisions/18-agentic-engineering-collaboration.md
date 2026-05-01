# Decision 18 · Agentic Engineering 协作原则

**日期**：2026-05-01
**状态**：已采纳

## 背景

触发来源：宝玉 X article《Karpathy 最新访谈：Vibe Coding 只是开始，真正重要的是 Agentic Engineering》
（2026-04-29），原文链接：
<https://x.com/dotey/status/2049617833370202182>

文章核心判断：Vibe Coding 提升软件创造下限；Agentic Engineering 的责任是
在使用 Agent 提速时，仍保住专业软件的质量、安全、可维护性和责任边界。

Fuxi 当前正进入多 agent、本地/远程节点、worktree 隔离、共享记忆与任务审计
并行展开阶段。此时不能把开发方式退化为“让 Agent 一路生成代码”；必须把
Agent 放入可验证、可审计、可回滚的工程流程。

## 决策

后续 Fuxi 开发按 **Agentic Engineering** 口径协作：

1. 用户负责目标、边界、价值判断和不可替代的领域理解。
2. Codex 负责把目标转成规格、实现、测试、审计证据和可 review 的提交。
3. 所有 Agent 产物必须进入隔离工作区，并留下可追溯的事件、diff、测试结果。
4. 任何“能跑但结构差”的代码都不算闭环；可维护性和安全性是交付标准的一部分。
5. 对 Agent 的信任来自验证，不来自流畅输出。

## 用户该做什么

- 给出业务目标和真实约束：要解决什么、为什么值得做、哪些行为不能破坏。
- 定义安全边界：数据、资金、身份、权限、远程节点、网络访问和文件写入的红线。
- 做产品和工程品味判断：什么是“自洽”、什么是“臃肿”、什么是不可接受的复杂度。
- 对关键方案做取舍：例如 workspace-first 还是 job-first，软沙箱还是硬沙箱默认开。
- 在 review 时盯高层语义：功能是否闭环，架构是否讲得通，是否留下未来债务。

用户不需要逐行指挥实现，也不需要替 Codex 选可逆的细节。

## Codex 该做什么

- 先读代码和文档，建立当前事实，不靠印象下结论。
- 把模糊目标落成明确规格、边界和验收条件。
- 主动实现，不停在“后续我会”；一次交付应当是可 review 的完整单位。
- 对所有改动提供验证：测试、lint、diff 检查、CI、事件/日志证据。
- 识别并修复文档漂移，让文档和代码保持同一个真相源。
- 对 Agent 生成代码保持怀疑：删冗余、压抽象、补测试、保留审计线索。
- 对无法闭环的点明确标注为缺口，不把“已有路径”包装成“已完成能力”。

## 我们之间如何协作

默认节奏：

1. 用户给目标和边界。
2. Codex 查现状，直接形成可执行方案。
3. Codex 实现、验证、提交、同步文档。
4. 用户 review 语义和方向。
5. Codex 根据 review 继续收口，直到功能、测试、文档和主线状态一致。

讨论架构时，优先回答四个问题：

- 工作区归属是否隔离？
- 行为是否可审计？
- 失败是否可恢复？
- 结果是否可验证？

## 对 Fuxi 的直接影响

近期开发优先级按此原则调整：

1. workspace-first：本地和远程 agent 都必须有明确 workspace/worktree 归属。
2. sandbox-first：默认软沙箱收拢 HOME/TMP/cache 到 worktree；硬沙箱按 role/node 策略开启。
3. audit-first：任务前后记录 status、diff、未跟踪文件、外部逃逸和测试结果。
4. recall-first：召回以 workspace context 为主，CLI session 只是附加能力。
5. dist 节点补齐 workspace 语义：远程 job 不能只是 node/job-first，必须能解释 repo、branch、worktree、node_id。

## 何时重审

- 远程节点开始支持真实 per-job worktree 后。
- 沙箱策略从软约束升级为默认硬隔离前。
- 引入 gemini/opencode 等新 adapter，且其 workspace/cwd 语义不同于 cc/codex 时。
- Agent 自动 review / red-team 成为主流程门禁时。

