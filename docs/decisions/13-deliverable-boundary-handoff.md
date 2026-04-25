# Decision 13 · 门客交付走 deliverable 边界 nudge，不抄送中间过程

**日期**：2026-04-25
**状态**：已拍板（用户 2026-04-25 明示选后者）；待 EventBus + 玄女订阅层实装

## 背景

40 分清单（B 路第 2 件"完善 agent 分布式节点"）讨论中，"门客给玄女交付"语义有
两种解：

- **A**（中间过程透传）：门客每完成一步都向玄女打包摘要、玄女作为编排者实时
  跟进。
- **B**（deliverable 边界 nudge）：门客中间过程**只**留痕 EventBus，玄女默认
  不主动消费这些事件；门客**仅在 deliverable 完成**时主动 ping 玄女审阅。

用户原话：「这个设计我倾向于后者，太多中间过程对玄女来说是噪声。」

## 决策

走 B：deliverable 边界 nudge 模式。

实装契约：

1. **新事件** `EventKind::AgentRequestReview { agent, task, deliverable_kind, summary, artifact_ref }`——门客主动呼叫玄女审阅时发的。这是**唯一**会主动占玄女 attention 的事件类型。
2. **中间事件继续按公理 2 抄送**（`AgentResponded` / `ToolCalled` / `ToolResult` 等照旧入 EventBus、写 SQLite、Firehose 看得到）。**但玄女订阅层默认不读取**，attention budget 留给 `AgentRequestReview`。
3. **玄女可主动 query**：用户/玄女想知道某门客现在在做啥，走 `fuxi status --id <agent>` / `recall(agent_id)` 拉过去事件——pull on demand，不 push。
4. **deliverable_kind 枚举**（初版）：`research_summary` / `code_change` / `test_result` / `decision_request` / `error_block`——后续可扩展。
5. **玄女的 system prompt 要更新**：明确"中间过程是噪声，门客找你 = 该看了；
   你不需要主动追门客进度"。配套 `fuxi-skills/skills/xuannv/dispatch-protocol.md`。

## 理由

1. **公理 2 不破，只是**重新定义"知情权"为"可查"而非"必读"——事件依然完整在
   SQLite，玄女想查随时能 recall，但不被 push 淹没。
2. **attention 是稀缺资源**——cc / opencode 实测：用户对玄女的体感"反应慢" / "
   啰嗦"全是因为 attention 被中间事件挤占。门客 47 步只让玄女看 2 个 deliverable，
   user-perceived latency 从分钟级到秒级。
3. **40 分清单需要**——IM 频道（手机）每条 push 都是震动，47 次震动 = 用户关
   通知 = fuxi 失声。门客 deliverable nudge = 一次震动恰好对应一个 user-actionable
   决策点。
4. **架构清爽**：玄女不再需要"过滤事件"逻辑——订阅层就只订 `AgentRequestReview`；
   想看中间过程就开 Firehose TUI（公理 1/2/3 不变，pulled 视图）。

## 代价

- 门客侧要会**判断"什么时候该 nudge"**——不能每写一个文件就 nudge（退化成 A），
  也不能憋到 task 全做完才 nudge（用户失去过程感）。`deliverable_kind` 5 类
  枚举给约束；具体由门客 system prompt 教（"完成一个调研主题→nudge / 完成一段
  能跑的代码→nudge / 卡住超过 N 步→nudge error_block"）。
- 玄女失去对中间过程的**实时**感知——若用户问"鲁班现在做到哪了？"玄女得现 recall。
  代价小：用户问的频率低，recall 一次几十毫秒。
- AgentRequestReview 事件**必须可靠传达**——若 push 失败，门客侧 task 卡死等审。
  需要 retry + 超时降级（超时落事件 `ReviewRequestTimeout` 玄女兜底处理）。

## 何时不适用 / 何时重审

- 若 deliverable_kind 5 类无法覆盖大量真实场景，得扩枚举 → 重审是否要更细分级
- 若用户实测发现"漏看了关键中间步" → 加 nudge 触发器（如 tool error 累计 N 次自动 nudge）
- 若多门客并发对玄女 nudge 风暴（10 个门客同时找她审）→ 加门客侧 nudge rate-limit
  + 玄女侧批量审视图

## 参考

- 公理 2（玄女永远有知情权）—— 此决策重新定义"知情权"语义
- 40 分清单（B 路第 2 件实质内容，写在 `project_fuxi_b_path_vision` memory）
- cc 的 teammate idle notification（`useTeammateShutdownNotification.ts`）类似机制
  的参照——cc 也是 pull on demand + 关键时主动 nudge
