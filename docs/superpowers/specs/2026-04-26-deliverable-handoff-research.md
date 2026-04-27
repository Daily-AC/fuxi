# 门客交付 deliverable handoff · 调研报告（β · #47）

**日期**：2026-04-27
**状态**：调研完成，待 team-lead 跟用户拍板后单开实装任务
**触发**：用户实测多次反馈「玄女给鲁班派活，发现鲁班都干完了但是玄女不知道也不知道干了什么」

## TL;DR

team-lead 派活描述里的 root-cause 推断**部分有误**：

> "单机本地 cc/codex 门客**没有发** AgentRequestReview，只有分布式 dist worker 路径发它"

**实际**：cc 适配器（`crates/fuxi-agent-cc/src/parser.rs:358-372`）和 codex 适配器（`crates/fuxi-agent-codex/src/parser.rs:339-353`）**都已实装** 决策 13 sentinel 解析。`bridge.rs` AgentRequestReview → 玄女 intervene 链路也通（带 retry + ReviewRequestTimeout 兜底）。`luban` skill 的 `instructions/deliverable-nudge.md` 也教 LLM 怎么发 sentinel JSON。

整条链路**代码层全部 wired**。但用户实测不工作 = LLM 实际**没有**在 turn 末尾输出 sentinel。问题不在「机制没接通」，在「LLM 没用机制」。这是**社会工程**问题（怎么让 cc 真的写 sentinel）而非纯架构问题。

下面三节按 team-lead 任务模板展开。

---

## §1 · 决策 13 现状盘点 + 单机为什么"看起来"没接通

### 1.1 sentinel 机制现状

**事件产物侧**（`crates/fuxi-core/src/event.rs:268`）：
```rust
EventKind::AgentRequestReview {
    agent: AgentId,
    task: TaskId,
    deliverable_kind: DeliverableKind,  // ResearchSummary/CodeChange/TestResult/DecisionRequest/ErrorBlock
    summary: String,
    artifact_ref: Option<String>,
}
```

**cc 适配器解析**（`crates/fuxi-agent-cc/src/parser.rs:358-372`）：

LLM 在 `AssistantText` 内输出**单独一行裸 JSON**：
```json
{"_fuxi":"request_review","kind":"code_change","summary":"feat(x): 三绿，等审 commit","artifact_ref":"sha:abc1234"}
```

`try_parse_request_review_sentinel` 严格判 `text.trim().starts_with('{')` + `_fuxi == "request_review"` + `summary` 非空 + `kind` 是合法 enum。命中后：
- 翻成 `AgentRequestReview` 事件入 EventBus
- **不**置 `responded_this_turn`（控制消息不算 LLM 回复）
- **吞掉**这一行（不再 emit `AgentResponded`，用户视图看不到 JSON）

**codex 适配器**（`parser.rs:339-353`）同款处理 `CodexEvent::AgentMessage`。

**bridge 转译**（`crates/fuxi-orchestrator/src/bridge.rs:388-440`）：

订阅 EventBus 看 `AgentRequestReview` → 构造 prompt：
```
[REVIEW_REQUEST] 门客 <agent_id>（role=<role>）呈递 deliverable_kind=<tag> 待审。

摘要：<summary>

附件：<artifact_ref>

[INSTRUCTION: 该门客主动找你审阅。判断是否接受 / 改派 / 让他续做，并向用户汇报或追问]
```
然后 `try_intervene_with_retry(intervener, xuannv_id, &prompt, &[200, 500, 1000])` 投递给玄女，**永远 interrupt** 模式（高优先级 attention 信号）。retry 全失败 → 发 `ReviewRequestTimeout` 兜底事件，bridge 自己又把这条转 intervene 给玄女让她主动 recall。

**门客 skill 教学**（`roles/luban/instructions/deliverable-nudge.md`）：

165 字节内含 5 类 deliverable_kind 的"该发 / 不该发"判断 + sentinel JSON 完整格式 + 防自己撞脚提示（必须行首裸 JSON、不能在 markdown 围栏里、自然语言"我要 nudge"不触发）。

### 1.2 为什么 team-lead "单机没接通" 的诊断错？

team-lead grep `AgentRequestReview` 看到主要命中点在 `dist.rs:4467`，误以为生产 publisher 只在分布式路径。但那行实际是 **测试断言**：

```rust
// crates/fuxi-cli/src/dist.rs:4454-4477
/// Decision 13 sentinel：`AssistantText` 行装 `_fuxi:request_review` JSON →
/// bus 拿到 AgentRequestReview（**不**是 AgentResponded——sentinel suppresses）。
#[tokio::test]
async fn cc_publish_line_routes_request_review_sentinel_to_bus() {
    // ...
    let drained = ...;
    assert_eq!(drained.len(), 1, "sentinel suppresses AgentResponded");
    match drained[0].kind { EventKind::AgentRequestReview { .. } => { /* ok */ } }
}
```

dist.rs 的 `cc_publish_line` 函数复用了**同一个 cc parser**——分布式 worker 跑 cc 时也走 `parser::translate`，sentinel 检测路径完全一致。本地 cc/codex agent（`crates/fuxi-agent-cc/src/agent.rs::ws_pump → translate`）走的是同一段代码。

### 1.3 真正的 root cause

整条链路代码 + skill 都到位，那为什么用户实测不工作？

**最可能原因（按发生概率排序）**：

#### A. LLM 不读 / 不信 / 不用 sentinel（社会工程问题）

luban skill 的 `instructions/deliverable-nudge.md` 是**按需读**的——`ROLE.md` 说"日常派活不必通读，需要细节时用 `Read` 读对应文件"。LLM dispatch 后看 ROLE.md 主体，主体里只有一句"何时呼叫玄女审阅（deliverable nudge） → `instructions/deliverable-nudge.md`"指针。LLM 实际可能**根本没去读这个指针**，更不会照办。

用户场景"看一下 ~/erp 的 git 分支" → 鲁班执行：
1. `Bash: cd ~/erp && git branch` → 拿到输出
2. `AssistantText: "当前分支是 feat/xxx，远程有 N 个分支..."` → 这是普通汇报，**不是** sentinel
3. cc 翻 `AgentResponded { text: "当前分支是..." }` → 入 EventBus，**不**触发玄女 intervene
4. cc `result` event → 翻 `TaskStateChanged { Done }` → 入 EventBus
5. dispatch pump 收尾 → shelf 标 Idle
6. 玄女**视角**：从未被 ping，唯一能看到的是用户问"鲁班怎么样了"时她去 recall

**这条路径里玄女确实"不知道也不知道干了什么"**——用户描述的现象正是。

#### B. sentinel 格式过严，LLM 自己撞脚

`try_parse_request_review_sentinel` 要求**整行裸 JSON**。LLM 习惯把代码包在 ```` ``` ```` 围栏里、行首加缩进、用 `"_fuxi": "request_review"`（带引号 escape 后行首仍是 `{`，OK）。

但常见失败模式：
- LLM 输出"我做完了，下面是 nudge：" + 换行 + sentinel → 如果 sentinel 在前面那行的尾部，trim 后不是行首 → 不命中
- LLM 把 sentinel 包在 ``` ``` 中（"请看下面的 review 请求"）→ markdown 围栏内的 `{...}` trim 后首字符不是 `{` 而是 backtick → 不命中（**这是设计意图**，避免"教学时讲到 sentinel 就触发"）
- LLM 在 sentinel 前后加自然语言注释同一段 → 整段 trim 后首字符是 `{` 但内容不是合法 JSON → `serde_json::from_str` 失败 → silent ignore

#### C. 玄女 system prompt 没教她"派活前要求 nudge"

`roles/xuannv/ROLE.md` 没看到「dispatch 时给鲁班说『干完用 sentinel 报』」的教学。玄女自己不知道这个机制，自然不会向鲁班强调。

### 1.4 单机 vs 分布式的真实差异

team-lead 觉得"分布式路径有，单机没有"，部分有事实基础——但不是 *AgentRequestReview 事件本身*：

- **分布式路径**：`fuxi dist worker` 把 cc 进程在远端节点跑，事件流通过 controller relay。中间过程事件经过 sentinel pattern**也**触发 AgentRequestReview（同一段 parser）
- **单机路径**：cc 适配器在本机进程跑，事件流走 `mpsc::channel → Fuxi::dispatch pump → bus.publish`。同样过同一段 parser 同样 sentinel 检测

唯一的真区别可能是 **远端 worker 启动时打过 `--system-prompt` 注入特殊指令**让 LLM 更倾向用 sentinel？需要 grep 确认（dist.rs 那段我没细读，本调研先 mark 为 known unknown）。

---

## §2 · cc 源码学到的（teammate 通信模型）

### 2.1 cc team 通信架构概览

cc 的 agent team 通信走 **file-based mailbox + lockfile**，跟 fuxi 的 EventBus push 模型不同：

| 组件 | 路径 | 用途 |
|---|---|---|
| `teammateMailbox.ts` | `~/.claude/teams/<team>/inboxes/<agent>.json` | inbox 文件，append-only message list |
| `lockfile` | `<inbox>.lock` | 并发写时的 file lock（max 10 retry, 5-100ms backoff） |
| `attachments.ts` | inbox → leader/teammate prompt 的 attachment | inbox unread → 注入下次 turn 的 system context |
| `inProcessRunner.ts` | in-process teammate 启动入口 | 用 `AsyncLocalStorage` 隔离 teammate context |
| `SendMessageTool` | LLM 工具 | 唯一对外发消息的入口（`{to, summary, message}`） |

### 2.2 关键代码片段

**消息 schema**（`teammateMailbox.ts:43-50`）：

```typescript
export type TeammateMessage = {
  from: string
  text: string
  timestamp: string
  read: boolean
  color?: string   // Sender's assigned color
  summary?: string // 5-10 word preview shown in UI
}
```

值得借鉴的两个字段：
- `read: boolean` —— 在 mailbox 里标记已读，避免重复注入
- `summary` —— 5-10 词预览，UI 显示用，**也**可以塞进 leader prompt 减少 token

**写入路径**（`teammateMailbox.ts:134-192`）：

```typescript
export async function writeToMailbox(recipientName, message, teamName?) {
  await ensureInboxDir(teamName)
  // 1. 创建 inbox 文件（'wx' 模式，已存在 fail-fast）
  await writeFile(inboxPath, '[]', { encoding: 'utf-8', flag: 'wx' })
  // 2. 加锁
  release = await lockfile.lock(inboxPath, { lockfilePath, ...LOCK_OPTIONS })
  // 3. 锁内 re-read（避免覆盖并发 append）
  const messages = await readMailbox(recipientName, teamName)
  messages.push({ ...message, read: false })
  // 4. 锁内 write
  await writeFile(inboxPath, jsonStringify(messages, null, 2), 'utf-8')
}
```

**system prompt 教学**（`teammatePromptAddendum.ts:8-18`）：

```
# Agent Teammate Communication

IMPORTANT: You are running as an agent in a team. To communicate with anyone on your team:
- Use the SendMessage tool with `to: "<name>"` to send messages to specific teammates

Just writing a response in text is not visible to others on your team -
you MUST use the SendMessage tool.

The user interacts primarily with the team lead.
```

**SendMessage tool prompt**（`SendMessageTool/prompt.ts:22-36`）：

```
# SendMessage

Send a message to another agent.

{"to": "researcher", "summary": "assign task 1", "message": "start on task #1"}

Your plain text output is NOT visible to other agents — to communicate, you MUST call this tool.
Messages from teammates are delivered automatically; you don't check an inbox.
```

**leader 收消息路径**（`attachments.ts:3590-3635`）：

```typescript
// Leader 每次 turn 开始前，readUnreadMessages 拉所有未读
const allUnreadMessages = await readUnreadMessages(agentName, teamName)
const unreadMessages = allUnreadMessages.filter(m => !isStructuredProtocolMessage(m.text))
// ...
// Combine with AppState.inbox messages, dedupe by from+timestamp+text
// 包成 attachment → 拼进下一个 prompt 给 LLM
```

### 2.3 cc 模型对 fuxi 的启示（取舍）

| cc 做法 | fuxi 等价 / 借鉴空间 |
|---|---|
| **强制工具调用**：plain text 不可见，必须用 `SendMessage` tool | fuxi 现在用 sentinel JSON 行——同款思路（强制结构化）但**没教学**到位 |
| **system prompt addendum**：每个 teammate 起手就读"你必须用 SendMessage" | fuxi 玄女派活时**没**注入"鲁班你必须用 sentinel 回报"——这是缺口 |
| **summary 字段** 5-10 词 preview | fuxi 的 `AgentRequestReview.summary` 已有，但 deliverable-nudge.md 教的是"一两句中文"，比 cc 的 5-10 词长 |
| **inbox unread → 自动注入下次 prompt** | fuxi 已通过 bridge intervene 注入玄女对话——架构等价 |
| **file lockfile 防并发** | fuxi 走 broadcast channel + SQLite WAL，原子性 by tokio + sqlite，无须 lockfile |
| **attachment 把消息注入 leader prompt 头** | fuxi 走 intervene → cc CLI 的 input stream，效果等价 |

**最大可借鉴的一条**：cc 把"你必须用 SendMessage"写在每个 teammate 起手的 **system prompt addendum** 里，不依赖 teammate 主动读 instructions/。fuxi 等价做法 = `Fuxi::dispatch` 时给门客 cc 的 prompt 末尾**自动 append** 一段 `[SYSTEM: 完成后必须用 sentinel JSON 回报；格式：{...}]`，让 LLM 没机会"忘了读"。

### 2.4 cc stream-json `result` 事件能识别"语义结束"吗？

可以但不够。`result` 事件结构（reference_cc_stream_json.md）：

```json
{"type":"result","subtype":"success",
 "duration_ms":..,"duration_api_ms":..,"num_turns":..,
 "result":"<最终文本>",
 "total_cost_usd":..,"usage":{..},
 "stop_reason":"end_turn","terminal_reason":"..."}
```

`result.result` 字段 = LLM 最终汇总文本。**问题**：这个字段的内容本身就是 `AssistantText` 流的最后一段（cc parser 已经识别这个，line 433-443 的"双发去重"逻辑就是处理这个字段）。它**不是** "我做完了" 的语义信号——纯粹是 LLM 自然说的话。

也就是说：`result` 事件提供「turn 在此终止」的边界，**但不提供** "LLM 主动声明这是 deliverable" 的语义。要识别 deliverable 必须依赖：
- (A) sentinel 模式（现有 D 方案，依赖 LLM 自觉）
- (B) 适配器 LLM 解析（用另一个 LLM 看 `result.result` 决定 deliverable_kind / summary）—— 加 LLM call 增成本 + 延迟
- (C) 把 `result` 整段当作隐式 deliverable，无脑 wrap → 等于回 A 模式（每个 task 完成都 ping 玄女）

---

## §3 · 候选方案对比 + 推荐

### 方案 A · cc 适配器 stream 末尾自动合成 AgentRequestReview

**做法**：在 cc parser 的 `ResultSuccess` 翻译路径里，除了 `TaskStateChanged{Done}` 之外，**自动**追加一条 `AgentRequestReview { deliverable_kind: ResearchSummary, summary: <最近 N 条 AssistantText 拼接>, artifact_ref: None }`。

**优点**：
- 零依赖 LLM 行为——任何门客 turn 完成都自动 ping 玄女
- 实装小：parser 改 ~10 行，bridge / event 已就绪

**缺点**（致命）：
- **完全违背决策 13** §B 原则。决策 13 明文："门客**仅在 deliverable 完成**时主动 ping 玄女"。每个 turn 完成 ≠ 一个 deliverable（鲁班可能一个任务跑 5 个 turn 才完）
- attention 模型崩溃：用户当时撞过的"反应慢/啰嗦"问题原地复现
- `deliverable_kind` 全填 `ResearchSummary` 失去语义区分价值

**裁决**：**否决**。除非用户/team-lead 显式说"决策 13 失败，回 A 模式"。

### 方案 B · 玄女主动 poll task_completed event，自己派 sub-task 让 cc 总结

**做法**：玄女订阅 EventBus 拿 `TaskStateChanged{Done}`（she 的视角看门客 idle），主动给鲁班发 sub-task "总结你刚才做了什么"。鲁班回复后玄女 recall 这条。

**优点**：
- 推理路径符合公理 2（玄女主动 query，pull on demand）
- 不需要 LLM 自觉 sentinel
- summary 由"想看时再产"，token 用得最准

**缺点**：
- 玄女额外承担"知道哪些 task 是她派的"逻辑——要查 task tree
- sub-task 来回额外加一次 cc 启动延迟（~3-6s 冷启）+ token cost
- 玄女自己变成 polling 角色——违反公理 3 的精神（虽然 EventBus subscribe 不是真 polling，但她自己产出"问鲁班"的行为是 reactive 不是 deliverable-driven）
- 用户体感的"玄女不知道"问题修了，但 token cost 翻倍

**裁决**：**保留**作为 v1.x 兜底（如果 D' 方案社会工程也失败的话）。

### 方案 C · task_completed 自动 wrap AgentRequestReview，bridge 翻给玄女

**做法**：与方案 A 几乎一样，但在 dispatch pump 层（不是 cc parser 层）做：pump 看到 `TaskStateChanged{Done}` → 自动发一条 `AgentRequestReview`。把适配器层不污染。

**优点**：
- 比 A 少污染 cc parser
- bridge / event 仍就绪

**缺点**：
- 仍违背决策 13（同 A）
- pump 层不知道 deliverable_kind，只能填一个默认值
- summary 拿不到（pump 看的是 EventKind 不是文本流），要么写"任务已完成（无摘要）"——空 ping 没价值

**裁决**：**否决**。同 A 否决理由 + 摘要拿不到。

### 方案 D · sentinel + 强制注入 dispatch prompt（推荐 ★）

**做法**：sentinel 机制保留**不动**（cc/codex parser + bridge + role skill 全存在）。**新增**一步：`Fuxi::dispatch(agent_id, task)` 在投递 `task.description` 给 cc 之前，**append 一段强制指令**：

```
---
[SYSTEM · deliverable handoff]

完成任务后你 **必须** 在最后一条 assistant message 中 **单独一行** 输出
sentinel JSON 通知玄女审阅。格式：

{"_fuxi":"request_review","kind":"<5 类之一>","summary":"<一两句中文>","artifact_ref":"<可选 commit sha / file path>"}

5 类 kind：research_summary / code_change / test_result / decision_request / error_block。

不输出 sentinel = 玄女不知道你做了什么 = 任务被认为没收尾。
若任务无 deliverable（如打个招呼），用 kind=research_summary + summary 简述。
---
```

**优点**：
- 完美符合决策 13（仍是 deliverable-driven，只是减少"LLM 忘了用"的概率）
- 改动最小：`Fuxi::dispatch` 加 ~15 行 prompt 拼接
- 任何门客（luban / luban-codex / 任何后续新 role）自动获得 nudge 教学
- 跟 cc team prompt addendum 思路同款（system prompt 强制 + 工具不调用 = 不可见），证明过的模式

**缺点**：
- `task.description` 会变长（但只多 200 字符，可忽略）
- LLM 可能仍不听话（但 cc team 实测中 SendMessage tool 教学成功率 ≥ 95%——同款套路）
- 玄女派活时如果用户原话本身**不是** task description（是聊天用 intervene），dispatch 路径不走，nudge 教学也不会注入。intervene 退化 dispatch 时（`Fuxi::intervene` 内部调 `dispatch`）路径会走到，OK；纯 intervene busy worker 时不走 dispatch → 不注入——属于**已知缺口**留 v1.x

**裁决**：**推荐**。最小改动 + 最贴决策 13 原则 + 借鉴了 cc team 验证过的"system prompt 强制" pattern。

### 方案 E · 玄女 dispatch 自定 sub-prompt（社会工程加强版 ★ alternative）

**做法**：跟 D 类似但**让玄女自己决定**——更新 `roles/xuannv/ROLE.md`，加一段「派活协议」：

```
## 派活协议

你给门客派活时（dispatch），task.description 末尾**必须**含一句：
「完成后用 sentinel JSON 报：{"_fuxi":"request_review","kind":"<...>","summary":"<...>"}」

否则门客可能不报，你会失去 attention。
```

**优点**：
- 不动 Rust 代码，纯 prompt 改动
- 玄女自己控制 → 灵活性高（特殊任务可以省略 nudge 要求）

**缺点**：
- 仍依赖玄女 LLM 自觉读 ROLE.md 这一段
- 更间接：玄女说 → 鲁班听 → 鲁班输出 sentinel，链路两层 LLM 都可能掉链
- 跟 D 不互斥，可叠加

**裁决**：**保留**作为 D 的补充。先做 D（硬保证），观察 1-2 周；若发现玄女想跳过 nudge 教学的合理场景，再加 E 的 ROLE.md 协议让玄女有控制权。

### 方案 F · sentinel 格式放宽（容错 fix）

**做法**：现有 `try_parse_request_review_sentinel` 太严（必须整行裸 JSON）。放宽为：扫描 `AssistantText` 全文按行匹配，任何**单行**符合 schema 都触发；markdown 围栏内仍排除。

**优点**：
- 修 LLM 易撞脚 case（前后多注释一行）
- 改动 cc/codex parser 各 ~10 行

**缺点**：
- 假阳性风险：LLM 在 explainer 里写 "下次我可能发 `{...}`" 也会触发——破坏「文档里写示例不触发」的设计意图
- 边界 case 复杂：行内换行符、CRLF、Unicode bidi 字符...

**裁决**：**条件保留**。先看 D 实装后假阴性率（LLM 多频写 sentinel 但 parser 漏识别）。如果 D + F 不交叉问题就先做 D；如果 D 后仍漏识别 → 加 F 但守住"markdown 围栏内不触发"。

### 推荐路径

按优先级：

1. **方案 D（必做）**：`Fuxi::dispatch` 注入 system prompt addendum 教 sentinel。最小改 + 最大杠杆 + 社会工程问题用社会工程方法解
2. **观察 1-2 周** —— 用户在 home 实测，看 sentinel 触发率
3. **方案 E（可选）**：若玄女 LLM 在 dispatch 时不自觉补 nudge 要求，更新 ROLE.md 加派活协议
4. **方案 F（条件做）**：若发现 sentinel 漏识别（LLM 写了但格式微偏） → 放宽 parser
5. **方案 B（兜底）**：若 D + E + F 都不解决 → 玄女主动 poll task_completed 自派 summary sub-task。这个是最后一招，token cost 高

**否决**：A、C（违背决策 13）

### 实装难度估算（仅 D）

- `crates/fuxi-orchestrator/src/fuxi.rs::dispatch` 在 `task.description` append addendum：~15 行
- 测试：`tests/dispatch.rs` 加 1 条 `dispatch_appends_sentinel_addendum_to_task_description`：~30 行
- 不改 EventKind / parser / bridge / role skill
- 可选：把 addendum 文本抽到 const、env override（`FUXI_DISABLE_SENTINEL_ADDENDUM=1` 关）方便实验

---

## 关联

- 决策 13（`docs/decisions/13-deliverable-boundary-handoff.md`）—— 本调研锚点
- 公理 2（玄女有知情权，无否决权）—— 推荐方案 D 完全兼容
- 公理 1（headless agent 不显式沟通 = 没做）—— sentinel 是显式沟通的载体，方案 D 加强它的可见性
- `roles/luban/instructions/deliverable-nudge.md` —— 现有教学，**保留**作为 deep-dive；ROLE.md 主体只能给 LLM 主动读
- `crates/fuxi-cli/src/dist.rs:4454` —— 分布式路径 sentinel 测试（同 parser 同行为）
- cc 源码 `src/utils/swarm/teammatePromptAddendum.ts` —— 方案 D 的灵感来源
- `reference_cc_stream_json` memory —— `result` 事件结构 + 不含 deliverable 语义的论证
