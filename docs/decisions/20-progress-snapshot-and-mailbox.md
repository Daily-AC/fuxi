# Decision 20 · 任务进度快照与 task-scoped mailbox

**日期**：2026-05-01  
**状态**：已实装首版

## 背景

对比 Claude Code agent team 后，Fuxi 保留自己的审计型 EventBus 架构，但吸收两点体验：

- 任务页需要直接看见“门客正在干什么”，不能只显示 busy/idle。
- 门客间通信可以存在，但必须 task-scoped、可持久化、可回放，不能绕过 EventBus 私聊。

## 决策

1. `/api/tasks` 的 `MemberCard` 物化进度快照字段：`phase`、`tool_use_count`、`last_activity`、`recent_activities`、`summary`。
2. 快照只从既有事件推导，不新增事实源：`ToolCallStarted` 计数并进入 recent，`ThinkingStarted/Finished` 推 phase，`AgentResponded` 产出 summary。
3. 新增 task-scoped mailbox 事件：`AgentMessageQueued`、`AgentMessageDelivered`、`AgentMessageRead`、`AgentMessageFailed`。
4. `Fuxi::send_agent_message` 是编排入口：先 queued，再尝试 `Agent::send_message`，成功 delivered，失败 failed 并返回错误。

## 质量标准

- 每条门客消息必须有 `task_id`、`from`、`to`、`message_id`。
- 失败也是事件，不允许“看起来发了但无审计”。
- UI 读快照字段；完整事实仍以 EventBus/SQLite 事件流为准。
- 远端节点后续接入时仍走同一事件 vocabulary，不新增旁路信箱。

## 当前边界

首版只完成协议、聚合、前端消费和本地编排入口。尚未把 mailbox 暴露为 IM API，也尚未把“门客必须用 SendMessage”注入到角色 prompt/tooling。后续接入时不得改变本决策的审计约束。
