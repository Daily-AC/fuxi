# v1.1 事件流 · 发布者/订阅者矩阵审计

## 1 · EventKind 变体全表 (39 个)

| # | 变体 | 发布者 crate/module | 订阅者位置 | meta.agent | meta.task | kind_tag | summarize | color_for |
|---|---|---|---|---|---|---|---|---|
| 1 | AgentSpawning | - | - | - | - | ✓ | ✓ | ✓ |
| 2 | AgentReady | fuxi-agent-cc::agent | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 3 | AgentShuttingDown | - | - | - | - | ✓ | ✓ | ✓ |
| 4 | AgentDead | fuxi-orchestrator::fuxi/bridge | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 5 | TaskCreated | fuxi-orchestrator::fuxi | repl/ingest | - | ✓ | ✓ | ✓ | ✓ |
| 6 | TaskDispatched | fuxi-orchestrator::fuxi | repl/ingest | - | ✓ | ✓ | ✓ | ✓ |
| 7 | TaskStateChanged | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 8 | TaskDelivered | - | - | - | - | ✓ | ✓ | ✓ |
| 9 | TaskBlocked | fuxi-agent-cc/codex::parser / orchestrator | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 10 | TaskResumed | fuxi-orchestrator::fuxi | repl/ingest | - | ✓ | ✓ | ✓ | ✓ |
| 11 | TaskCancelled | - | - | - | - | ✓ | ✓ | ✓ |
| 12 | MessageSent | - | - | - | - | ✓ | ✓ | ✓ |
| 13 | MessageReceived | - | - | - | - | ✓ | ✓ | ✓ |
| 14 | UserPrompted | fuxi-orchestrator::bridge / repl::ingest | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 15 | AgentResponded | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 16 | ThinkingStarted | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 17 | ThinkingFinished | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 18 | ToolCallStarted | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 19 | ToolCallFinished | fuxi-agent-cc/codex::parser | repl/ingest | ✓ | ✓ | ✓ | ✓ | ✓ |
| 20 | UserInterventionSent | fuxi-orchestrator::fuxi | dispatch.rs test | ✓ | - | ✓ | ✓ | ✓ |
| 21 | AgentInterrupted | - | - | ✓ | - | ✓ | ✓ | ✓ |
| 22 | TaskInterventionApplied | - | - | - | ✓ | ✓ | ✓ | ✓ |
| 23 | OrchestratorCcReceived | fuxi-orchestrator::fuxi/bridge | - | ✓ | - | ✓ | ✓ | ✓ |
| 24 | ConversationTransferred | - | - | - | - | ✓ | ✓ | ✓ |
| 25 | ConversationHandoffRequested | fuxi-orchestrator::fuxi | repl/ingest | ✓ | - | ✓ | ✓ | ✓ |
| 26 | ConversationReturned | - | - | - | - | ✓ | ✓ | ✓ |
| 27 | TriggerRegistered | fuxi-cli::daemon | - | - | - | ✓ | ✓ | ✓ |
| 28 | TriggerFired | fuxi-scheduler::keeper | - | - | - | ✓ | ✓ | ✓ |
| 29 | TriggerDispatched | - | - | - | - | ✓ | ✓ | ✓ |
| 30 | TriggerSkipped | - | - | - | - | ✓ | ✓ | ✓ |
| 31 | TriggerFailed | - | - | - | - | ✓ | ✓ | ✓ |
| 32 | PlatformStarted | fuxi-cli::up/repl/daemon | repl/ingest | - | - | ✓ | ✓ | ✓ |
| 33 | PlatformStopping | fuxi-cli::up | - | - | - | ✓ | ✓ | ✓ |
| 34 | SkillStaged | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 35 | SkillApproved | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 36 | SkillRejected | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 37 | SkillActivated | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 38 | NoRoleMatched | fuxi-cli::ipc (from lark-mcp) | - | - | - | ✓ | ✓ | ✓ |
| 39 | Custom | fuxi-agent-*/parser (fallback) | - | - | - | ✓ | ✓ | ✓ |

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

### Publisher-Orphan (定义但无人发)
1. **AgentSpawning** — 定义在 event.rs，但无发布点（可能在 agent spawn 层外部处理）
2. **AgentShuttingDown** — 同上，可能是预留位
3. **TaskDelivered** — 预留；当前用 TaskStateChanged (Done) 替代
4. **TaskCancelled** — 预留；无发布侧
5. **MessageSent** — 定义但未使用（A2A 消息可能走了别的通道）
6. **MessageReceived** — 同上

### Subscriber-Orphan (发布但无人订阅处理)
1. **TriggerDispatched** — 发布但未见 match 处理
2. **TriggerSkipped** — 同上
3. **TriggerFailed** — 同上（scheduler 发，无订阅方处理）
4. **AgentInterrupted** — 发布但无 match 处理
5. **TaskInterventionApplied** — 同上
6. **OrchestratorCcReceived** — 发布但无业务逻辑 match（仅作信息记录）
7. **ConversationTransferred** — 预留，未发布亦无订阅
8. **ConversationReturned** — 同上
9. **PlatformStopping** — 发布但无特殊订阅处理

## 4 · 重点发现

1. **TaskCreated/TaskDispatched 缺 meta.agent** — orchestrator 发这两个事件时未填 agent 字段，导致 repl 的 ingest handler 无法追踪"哪个玄女发的任务"。影响 TUI 的"归属感"显示。

2. **Scheduler 事件岛** — TriggerRegistered/TriggerFired/TriggerDispatched/TriggerSkipped/TriggerFailed 五个变体已在 kind_tag/summarize/color_for 中完整定义，但上游 scheduler 的发布与下游 orchestrator 的订阅之间无明确 wire，可能导致任务派发延迟或无声失败。

3. **MessageSent/MessageReceived 冷冻** — 这两个变体在 event 词汇表中但从未被触发或订阅，是设计债——可能原计划用于 A2A 通信审计但后来废弃，未清理。

---

## 审计方法

- **kind_tag 对齐** (/crates/fuxi-events/src/store.rs:233 与 fuxi-firehose/src/hub.rs:284 与 tui.rs:528)：编译时 exhaustive match，无漏。
- **summarize/color_for** (tui.rs:354/474)：exhaustive match，覆盖全。
- **publisher 定位** (grep `kind: EventKind::\w+` + context)：跨 orchestrator/agent-cc/agent-codex/daemon/keeper。
- **subscriber 定位** (grep `EventKind::\w+ =>` 在 repl/bridge/tests)：多数落在 repl.rs/ipc.rs 的 match 分支，或 hub.rs 测试。

