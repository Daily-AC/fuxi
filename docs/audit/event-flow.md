# v1.1 事件流 · 发布者/订阅者矩阵审计

> [!WARNING]
> `historical`：此审计基于 2026-04-20 附近的旧事件模型，保留用于追溯，不再代表当前 EventKind 全量矩阵。
> 当前架构状态以 `docs/status/now.md` 和代码为准。

> **2026-04-20 M3.6 更新**：删除 4 个真孤儿（TaskDelivered / TaskCancelled / MessageSent / MessageReceived），
> 现总变体数 **35**（原 39）。`AgentSpawning`/`AgentShuttingDown` 经实测确为有发布点，
> 不是孤儿（旧审计结论过时）。AgentInterrupted 颜色独立成 LightRed 警告色。

## 1 · EventKind 变体全表 (35 个)

| # | 变体 | 发布者 crate/module | 订阅者位置 | meta.agent | meta.task | kind_tag | summarize | color_for |
|---|---|---|---|---|---|---|---|---|
| 1 | AgentSpawning | fuxi-orchestrator::fuxi (spawn_worker / spawn_worker_in_worktree / register_managed_agent) | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 2 | AgentReady | fuxi-agent-cc::agent | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 3 | AgentShuttingDown | fuxi-orchestrator::idle_gc / fuxi (shutdown_agent / shutdown) | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 4 | AgentDead | fuxi-orchestrator::fuxi/bridge | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 5 | TaskCreated | fuxi-orchestrator::fuxi | repl/ingest | - | ✓ | ✓ | ✓ | ✓ |
| 6 | TaskDispatched | fuxi-orchestrator::fuxi | repl/ingest | - | ✓ | ✓ | ✓ | ✓ |
| 7 | TaskStateChanged | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 8 | TaskBlocked | fuxi-agent-cc/codex::parser / orchestrator | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 9 | TaskResumed | fuxi-orchestrator::fuxi | repl/ingest | - | ✓ | ✓ | ✓ | ✓ |
| 10 | UserPrompted | fuxi-orchestrator::bridge / repl::ingest | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 11 | AgentResponded | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 12 | ThinkingStarted | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 13 | ThinkingFinished | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 14 | ToolCallStarted | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 15 | ToolCallFinished | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 16 | UserInterventionSent | fuxi-orchestrator::fuxi | dispatch.rs test | ✓ | - | ✓ | ✓ | ✓ |
| 17 | AgentInterrupted | - | firehose/tui (LightRed 警告色) | ✓ | - | ✓ | ✓ | ✓ |
| 18 | TaskInterventionApplied | - | firehose/tui (兜底渲染) | - | ✓ | ✓ | ✓ | ✓ |
| 19 | OrchestratorCcReceived | fuxi-orchestrator::fuxi/bridge | firehose/tui (兜底渲染) | ✓ | - | ✓ | ✓ | ✓ |
| 20 | ConversationTransferred | - (M4.3 决定 D14) | - | - | - | ✓ | ✓ | ✓ |
| 21 | ConversationHandoffRequested | fuxi-orchestrator::fuxi | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 22 | ConversationReturned | - (M4.3 决定 D14) | - | - | - | ✓ | ✓ | ✓ |
| 23 | TriggerRegistered | fuxi-cli::daemon | firehose/tui | - | - | ✓ | ✓ | ✓ |
| 24 | TriggerFired | fuxi-scheduler::keeper | firehose/tui | - | - | ✓ | ✓ | ✓ |
| 25 | TriggerDispatched | fuxi-scheduler | firehose/tui (兜底渲染) | - | - | ✓ | ✓ | ✓ |
| 26 | TriggerSkipped | fuxi-scheduler | firehose/tui (兜底渲染) | - | - | ✓ | ✓ | ✓ |
| 27 | TriggerFailed | fuxi-scheduler | firehose/tui (兜底渲染) | - | - | ✓ | ✓ | ✓ |
| 28 | PlatformStarted | fuxi-cli::up/repl/daemon | repl/ingest | - | - | ✓ | ✓ | ✓ |
| 29 | PlatformStopping | fuxi-cli::up | firehose/tui (兜底渲染) | - | - | ✓ | ✓ | ✓ |
| 30 | SkillStaged | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 31 | SkillApproved | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 32 | SkillRejected | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 33 | SkillActivated | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 34 | NoRoleMatched | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 35 | Custom | fuxi-agent-*/parser (fallback) | - | - | - | ✓ | ✓ | ✓ |

## 2 · 对齐 Gap

### 缺 meta.agent 设置的 (发布侧未填)
- TaskCreated (5): 定义了 task_id 但未设 agent
- TaskDispatched (6): 未设 agent
- TaskResumed (10): 未设 agent
- TriggerRegistered (27): platform event，无 agent
- TriggerFired (28): 无 agent
- PlatformStarted (32): platform event，无 agent
- PlatformStopping (33): 无 agent
- SkillStaged/Approved/Rejected/Activated/NoRoleMatched (34-38): 从 IPC 代理而来，无明确 agent

### 缺 kind_tag 处理的
**无**——三处 kind_tag 函数已 exhaustive-match，全覆盖。

### 缺 summarize/color_for 的
**无**——tui.rs 已 exhaustive-match，全覆盖。

## 3 · 孤儿事件

### Publisher-Orphan (定义但无人发) —— 2026-04-20 M3.6 清理后

**已删（4 个，无回滚——bus payload 词汇表瘦身）**：
1. ~~**TaskDelivered**~~ — 真孤儿，删；终态走 `TaskStateChanged{to: Done}`
2. ~~**TaskCancelled**~~ — 真孤儿，删；终态走 `TaskStateChanged{to: Cancelled}`
3. ~~**MessageSent**~~ — 设计早期占位，A2A 通信不走 EventKind
4. ~~**MessageReceived**~~ — 同上

**实测后撤回的"假孤儿"（旧审计文档过时）**：
- **AgentSpawning** — `fuxi-orchestrator::fuxi` 三处发布（spawn_worker / spawn_worker_in_worktree / register_managed_agent）
- **AgentShuttingDown** — `fuxi-orchestrator::idle_gc` + `fuxi.rs::shutdown_agent` + `fuxi.rs::shutdown` 都发

### Subscriber-Orphan (发布但无人订阅业务处理) —— M3.6 决议

**保留不动，TUI 兜底渲染足矣**：
1. **TriggerDispatched / TriggerSkipped / TriggerFailed** — scheduler 发，TUI/audit 流式渲染兜底，无需新 match handler
2. **OrchestratorCcReceived** — 仅信息记录，TUI 已能渲染
3. **PlatformStopping** — fuxi-cli::up 关机标记，TUI 已能渲染
4. **AgentInterrupted** — TUI 升级为 LightRed 警告色（color_for 单独抽出）
5. **TaskInterventionApplied** — TUI summarize 已清晰

**等 M4.3 决定 D14 再处理**：
6. **ConversationTransferred** — 让贤场景，先冻结
7. **ConversationReturned** — 同上

## 4 · 重点发现

1. **TaskCreated/TaskDispatched 缺 meta.agent** — orchestrator 发这两个事件时未填 agent 字段，导致 repl 的 ingest handler 无法追踪"哪个玄女发的任务"。影响 TUI 的"归属感"显示。

2. **Scheduler 事件岛** — TriggerRegistered/TriggerFired/TriggerDispatched/TriggerSkipped/TriggerFailed 五个变体已在 kind_tag/summarize/color_for 中完整定义，但上游 scheduler 的发布与下游 orchestrator 的订阅之间无明确 wire，可能导致任务派发延迟或无声失败。

3. ~~**MessageSent/MessageReceived 冷冻**~~ — **M3.6 已删**。设计债清零。

---

## 审计方法

- **kind_tag 对齐** (/crates/fuxi-events/src/store.rs:233 与 fuxi-firehose/src/hub.rs:284 与 tui.rs:528)：编译时 exhaustive match，无漏。
- **summarize/color_for** (tui.rs:354/474)：exhaustive match，覆盖全。
- **publisher 定位** (grep `kind: EventKind::\w+` + context)：跨 orchestrator/agent-cc/agent-codex/daemon/keeper。
- **subscriber 定位** (grep `EventKind::\w+ =>` 在 repl/bridge/tests)：多数落在 repl.rs/ipc.rs 的 match 分支，或 hub.rs 测试。
