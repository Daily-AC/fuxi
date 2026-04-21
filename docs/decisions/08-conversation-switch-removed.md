# Decision 08 · 让贤（ConversationSwitch）拆除

**日期**：2026-04-21
**状态**：已采纳；**override Decision 05**

## 背景

M4.3 到期兑现 Decision 05 里留的延期承诺——激活 or 拆。

用户原话（2026-04-21）：

> 那好像没必要吧，我现在门客都可以介入呀，而且还有抄送。这就够了吧。

审视：
- 玄女 `intervene(target, text)` 可插任何门客（公理 #2 知情权）
- `OrchestratorCcReceived` 抄送保证玄女全知
- REPL 的 `@agent` 命令面板（M4.4）让用户手切 active target
- 上述三条合起来覆盖「切对话对象」全部现实场景

让贤 `ConversationHandoffRequested` 唯一独特补位：**门客自己主动发起主对话权切换**。v1.1 无能胜任此动作的门客：
- 铸牒司（skill creation 门客）没实装
- pm（需求澄清接收方）没实装
- 鲁班不会说"这事我不擅长"主动让位

激活 ≠ 0 调用变 1 调用；激活 = 把 dead EventKind 换成 dead CLI + dead skill 教学文。成本 ~1 天，收益为负。

## 决策

**拆**。删：
- `EventKind::ConversationTransferred`
- `EventKind::ConversationHandoffRequested`
- `EventKind::ConversationReturned`
- `Fuxi::request_handoff()` API
- `fuxi-events/store.rs` + `fuxi-firehose/hub.rs` 的 `kind_tag` 3 行
- `fuxi-firehose/tui.rs` 的 `summarize` 3 arm + `color_for` 3 variant
- `fuxi-cli/repl.rs` 的 ingest active-switch handler + narrate arms + 两个相关 test
- `fuxi-orchestrator/tests/dispatch.rs` 的 `conversation_handoff_requested_event_serializes`
- `roles/xuannv/instructions/dispatch-protocol.md` 的「让贤事件」段

## 为什么 override Decision 05

Decision 05 的三条保留理由逐条失效：

1. **"三独创 narrative"**——narrative 不能靠 dead code 支撑。真独创是 intervene + 抄送 + 真实时 Firehose 三件已落地的。让贤从未参赛
2. **"删了难加回来"**——加回来的成本（EventKind 同步 5 处）== 现在激活的成本。等到 v1.2 真有铸牒司场景再加，那时才有真实用例指导 API 形状
3. **"风险可控（dead code 没副作用）"**——反的：未被调用的公开 API 是认知负担，新 contributor 读代码会猜"这东西在什么时候触发"

## 何时重审

v1.2 实装铸牒司门客（skill creation）且确认其生命周期需要「主动让贤给 user 审核 → user 批准后返回玄女」场景时，重新设计 handoff API。那时 API 形状由真实场景决定，不是先验猜测。

## 代价

- 失去一份「已写好的差异化卖点」—— 毕设答辩时差一条 bullet。但 narrative 这东西不怕少一条，怕注水
- v1.2 若真需要让贤，要重新走一遍 EventKind 同步流程（~1h 工作）

## 参考

- Decision 05（被 override）· 2026-04-19 保留 wire 的原始理由
- architecture-v1.md §M1.5「让贤（ConversationSwitch）实装」—— 旧蓝图描述，已不再适用
- architecture-v1.1-roadmap.md §M4.3 「D14 让贤决策」—— 本 decision 兑现
