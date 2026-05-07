# 论文图表与代码呈现修改建议

审查目标：降低论文的“源码工程文档”观感，使第 3、4 章更像系统研究论文。原则不是删除实现细节，而是把真实代码从“正文主叙事”降级为“关键证据”，用图、伪代码和表格承担机制解释。

## 总体判断

当前论文放 Rust 代码本身没有格式问题。系统实现类论文可以展示真实代码，尤其是协议 wire 类型、状态枚举、序列化约束、数据库 schema 等“实现即契约”的内容。

真正的问题是比例与位置：当前稿件约有 11 张图、10 张表、20 段代码清单，其中第 4 章连续出现多段 30--60 行 Rust 控制流。读者容易把它读成“实现说明书”，而不是“系统设计与实验论文”。修改方向应是：

1. 保留少量短小真实代码，用来证明协议和类型边界。
2. 把控制流、调度流程、重试流程、SSE 解析流程改成伪代码。
3. 增加 1--2 张系统流程图，让图承担跨模块闭环解释。
4. 删除第 4 章开头“真 Rust 代码”这类自我定位，改为“关键实现片段与算法描述”。

## 优先级 P0：必须先改的地方

### 1. 改写第 4 章定位，避免主动声明“贴真实代码”

位置：`deliverables/thesis-v3/chapters/04-implementation.tex` 开头。

当前注释和导语都在强化“真 Rust 代码（贴自 fuxi 主仓）”的叙事。这会把读者预期引向源码 walkthrough。

建议改法：

- 删除或改写注释中的“真 Rust 代码（贴自 fuxi 主仓）”。
- 第一段不要强调“代码片段均直接节选”“不修改控制流”。
- 改成：本章围绕关键执行路径展开，用架构图、算法描述和少量实现片段说明事件总线、任务派发、A2A binding 与跨节点通讯如何形成闭环。

建议口径：

> 本章从系统执行路径出发，说明 Fuxi 如何将第二、三章的架构约束落到实现。为避免源码级展开淹没设计逻辑，正文仅保留协议契约、状态机与持久化边界等必要实现片段；调度、广播、SSE 流处理等过程性逻辑以算法描述和执行时序图呈现。

### 2. 第 4 章新增“端到端执行闭环图”

建议新增在 `chapters/04-implementation.tex` 的第 4 章导语之后，或放在“任务派发主循环”小节之前。

图名建议：

- 图 4.1 Fuxi 端到端任务执行闭环
- 或：图 4.1 从用户任务到多端观测的执行时序

图中至少包含这些节点：

- User / IM / REPL
- Xuannv Orchestrator
- Shelf / Worker Registry
- Worker Agent
- A2A JSON-RPC endpoint
- EventBus broadcast path
- SQLite WAL replay path
- Firehose / SSE / WebSocket / IM observers
- Workspace sandbox

图的核心信息不是模块清单，而是路径：

1. 用户提交任务。
2. 编排器创建 task 并选择 worker。
3. worker 在 sandbox 中执行。
4. worker 通过 A2A 或内部事件流回报状态。
5. EventBus 先广播、后持久化。
6. 观测端实时订阅，断线后从 SQLite WAL replay。
7. 若进入 `input-required`，状态映射到 `ShelfStatus::AwaitingInput`，用户回复后回到 `Working`。

验收标准：读者只看这张图，也能理解第 4 章所有代码片段服务于同一个闭环。

## 优先级 P1：代码清单替换为伪代码

### 3. 将 `dispatch` 主循环改为 Algorithm

位置：`chapters/04-implementation.tex`，当前清单：

- `lst:dispatch-main`
- 标题：`dispatch 本地路径主循环`
- 长度约 66 行

问题：这是当前最像源码文档的片段。它包含大量 Rust 异步、错误处理、match 分支和局部变量，读者需要先读 Rust 才能理解调度语义。

建议处理：

- 不保留完整 Rust。
- 改成“算法 1：本地任务派发流程”。
- 只保留输入、状态更新、worker claim、dispatch 调用、pump 启动、失败回滚。

伪代码应表达这些关键步骤：

```text
Input: task, optional target role, optional pinned node
1. persist TaskCreated event
2. if task is pinned to remote node, enqueue via DistEnqueuer and return
3. select an idle worker by role and saturation
4. atomically claim worker and mark it busy
5. emit TaskDispatched
6. call Agent::dispatch(task)
7. spawn a pump that republishes worker events to EventBus
8. on completion, release worker and update task state
9. on error, emit failure event and release worker
```

正文解释要强调三点：

- 原子 claim 避免并发任务抢同一 worker。
- pump 是事件流与编排层解耦的关键。
- dispatch 同时服务单节点与跨节点路径，但跨节点细节通过 trait 注入。

### 4. 将 `A2A server 单入口分发与 SSE 升级` 改为 Algorithm

位置：`chapters/04-implementation.tex`，当前清单：

- `lst:a2a-dispatch`
- 长度约 47 行

问题：该代码的学术价值在“单入口 JSON-RPC 分发 + 流式方法升级为 SSE”，不是具体 Rust handler 写法。

建议处理：

- 改成“算法 2：A2A JSON-RPC 单入口分发”。
- 保留方法名映射表或短表，不贴完整 handler。

伪代码应包含：

```text
Input: JSON-RPC request {method, params, id}
1. parse JSON-RPC envelope
2. match method:
   - agent/getCard -> return AgentCard
   - tasks/send -> create task and return Task
   - tasks/sendSubscribe -> create task and return SSE stream
   - tasks/get -> query task state
   - tasks/cancel -> cancel task
3. if method is streaming, encode task updates as SSE frames
4. otherwise encode JSON-RPC response
5. on parse or method error, return JSON-RPC error object
```

正文强调：Fuxi 当前沿用早期 A2A JSON-RPC binding；当前官方 PascalCase 方法集已在第 3、4 章说明，不要在伪代码里重新制造“v1.0 完全兼容”的暗示。

### 5. 将 `A2A client SSE 帧解析` 改为状态机图或伪代码

位置：`chapters/04-implementation.tex`，当前清单：

- `lst:a2a-sse-parse`
- 长度约 48 行

问题：逐行 SSE 解析代码对论文贡献较弱，且读者会被 `bytes_stream`、buffer、JSON parse 错误分支拖走。

建议处理：

- 优先改成一个小状态机图。
- 若不画图，则改成“算法 3：SSE 事件帧解析”。

状态机节点：

- Read chunk
- Accumulate buffer
- Split complete SSE frame
- Parse event type
- Decode task update
- Emit local event
- Preserve incomplete suffix
- Error frame / malformed JSON

正文保留一句：实际实现通过 streaming buffer 支持跨 chunk 帧，不要求每个 SSE frame 与网络 chunk 对齐。这一句比贴完整 Rust 更有学术价值。

### 6. 将 `HMAC-SHA256 验签实现` 缩为公式 + 伪代码

位置：`chapters/04-implementation.tex`，当前清单：

- `lst:hmac-verify`
- 长度约 36 行

问题：验签代码属于安全工程常规实现，完整 Rust 代码不增加论文贡献。

建议处理：

- 用公式表示签名：

```text
sig = HMAC_SHA256(secret, timestamp || "." || body)
```

- 再用 6--8 行伪代码说明：

```text
1. read timestamp and signature headers
2. reject if timestamp outside allowed skew
3. compute expected HMAC over timestamp and body
4. compare with constant-time equality
5. reject replay or malformed signature
```

正文强调安全边界：防篡改、防重放、常量时间比较。

## 优先级 P1：真实代码应保留但压短

### 7. 保留 wire 类型、状态枚举、schema 类短代码

这些代码建议保留，因为它们是“实现即契约”：

- `lst:taskstate`
- `lst:agentcard`
- `lst:jsonrpc`，但建议压短
- `lst:event-types`，但建议只展示 8--12 个代表性 `EventKind`
- `lst:event-schema`
- `lst:a2a-part-tagged`
- `lst:a2a-task-state`

处理原则：

- 每段控制在 10--20 行。
- 删除无关字段，用 `// ...` 折叠。
- caption 不写“当前实现完整代码”，写“核心 wire 形态”或“节选”。
- 每段代码后必须有一段抽象解释，不要只解释 Rust 语法。

### 8. EventBus 相关代码避免重复

当前第 3 章已有 `lst:eventbus-publish-snippet`，第 4 章又有 `lst:eventbus-publish`，二者语义重复。

建议：

- 第 3 章保留短节选，用来说明模块设计。
- 第 4 章删除完整 Rust 版本，改为算法或流程图。
- 若必须保留第 4 章代码，只保留 15 行以内，突出 `try_send`、后台 writer、lag sentinel 三个关键点。

正文需要把重点从“代码怎么写”改为“为什么这个流程同时满足不阻塞与不丢审计日志”。

## 优先级 P2：新增图表增强学术表达

### 9. 新增“事件总线双路径图”

若只加一张图，优先加第 4 章端到端闭环图。若还能加第二张，建议加事件总线双路径图。

位置：第 3 章事件总线模块或第 4 章事件实现小节。

图中展示：

- Publisher
- Broadcast Channel
- Live Subscribers
- Writer Queue
- SQLite WAL
- Replay API
- Lag Sentinel

图的核心对比：

- live path：低延迟、可丢旧帧、服务实时 UI。
- replay path：持久化、可补齐、服务审计一致性。

这张图能替代部分 `EventBus::publish` 代码，并直接支撑“真实时与不丢消息”这组设计矛盾。

### 10. 新增或改造“A2A 与 Fuxi 状态映射表”

位置：第 3 章 A2A 模块或第 4 章 A2A 实现小节。

表头建议：

| A2A 当前规范语义 | 早期 binding wire 值 | Fuxi 内部状态 | 是否终态 | 平台行为 |
|---|---|---|---|---|

至少包含：

- `TASK_STATE_SUBMITTED`
- `TASK_STATE_WORKING`
- `TASK_STATE_INPUT_REQUIRED`
- `TASK_STATE_AUTH_REQUIRED`，标注当前未实现
- `TASK_STATE_COMPLETED`
- `TASK_STATE_FAILED`
- `TASK_STATE_CANCELED`
- `TASK_STATE_REJECTED`，标注终态，当前未实现

这张表可以进一步降低前几轮已经修过的 A2A 事实风险，也能让 `InputRequired` 贡献更清晰。

## 伪代码排版建议

当前 `main.tex` 已有 `listings`，但没有算法环境。建议二选一：

### 方案 A：使用 `algorithm2e`

适合中文论文，排版紧凑，能编号为“算法”。

在 `main.tex` 加：

```tex
\usepackage[ruled,vlined,linesnumbered]{algorithm2e}
\SetAlgorithmName{算法}{算法}{算法清单}
\SetKwInput{KwInput}{输入}
\SetKwInput{KwOutput}{输出}
```

使用时注意中文模板是否已经定义算法浮动体；如果学校模板冲突，改用方案 B。

### 方案 B：继续使用 `lstlisting`，定义 Pseudocode 语言

风险更低，改动小。可定义：

```tex
\lstdefinelanguage{Pseudo}{
  morekeywords={Input,Output,if,then,else,for,while,return,emit,spawn,await,match},
  morecomment=[l]{//}
}
```

caption 写“算法 1：……”。缺点是不会自动进入算法目录，但对硕士论文通常够用。

建议优先方案 B，除非模板已经支持 algorithm 环境。原因：当前论文已稳定编译，临近提交不应引入新的浮动体兼容风险。

## 章节级修改目标

### 第 1 章

保持现状。第 1 章已有 overview 图，暂不需要加代码或技术细节。

### 第 2 章

保持架构图与公式为主。不要新增代码。第 2 章承担系统设计抽象，不应被实现细节污染。

### 第 3 章

定位：模块设计。

建议保留少量短 Rust 代码，因为第 3 章讨论模块边界与类型契约。需要压缩重复代码：

- `Part` serde tagged union：保留。
- `TaskState`：保留。
- `EventBus::publish`：保留短节选或改为图，不要与第 4 章重复。
- `dispatch pump`：建议改为伪代码或移到第 4 章统一讲。
- `CcAgent::launch_with_id`：建议缩短，重点讲 WS 反连握手，不展示完整 launch 代码。

### 第 4 章

定位：关键路径实现。

第 4 章应改成“图 + 算法 + 少量契约代码”的组合，而不是“连续源码清单”。

建议最终结构：

1. 第 4 章导语：说明本章只保留关键实现片段。
2. 端到端执行闭环图。
3. 事件模型与审计日志：短代码 + 表。
4. EventBus 非阻塞发布：算法或双路径图。
5. 本地 dispatch：算法。
6. A2A binding：短 wire 代码 + 方法映射表。
7. SSE 流处理：状态机图或算法。
8. 跨节点安全：公式 + 伪代码。

### 第 5 章

保持现状。第 5 章图已经足够，实验图数量和图表密度比第 3、4 章健康。

## 不建议做的事

1. 不要把所有 Rust 代码都删掉。这样会削弱“系统确实实现”的可信度。
2. 不要把真实代码机械翻译成逐行伪代码。伪代码应表达机制，不表达语法。
3. 不要新增装饰性架构图。每张图都必须替代一段难读代码或支撑一个论证闭环。
4. 不要引入大段彩色 GPT 风格插画。学术论文图应干净、低饱和、少文字、模块边界明确。
5. 不要改动第 5 章实验数据与结论。当前问题是呈现形态，不是实验重写。

## 验收标准

修改完成后应满足：

1. 第 4 章不再连续出现 30 行以上真实 Rust 控制流。
2. 真实代码清单总数从约 20 段降到 10--12 段左右。
3. 最长代码清单不超过 25 行，例外只能是表格或 schema。
4. 至少新增 1 张端到端闭环图。
5. `dispatch`、A2A server 分发、SSE parsing、HMAC verify 至少 3 个改成伪代码/图/公式。
6. 第 4 章导语不再把论文自我定位为“贴主仓真实代码”。
7. 全文仍能编译生成 `main.pdf`，且交叉引用无 undefined reference。

## 给 cc 的执行顺序

1. 先改第 4 章导语。
2. 新增端到端闭环图，并在第 4 章开头引用。
3. 把 `lst:dispatch-main` 改为伪代码。
4. 把 `lst:a2a-dispatch` 改为伪代码。
5. 把 `lst:a2a-sse-parse` 改为状态机图或伪代码。
6. 把 `lst:hmac-verify` 改为公式 + 伪代码。
7. 回头压缩保留代码清单，删除第 3、4 章重复的 EventBus 完整实现。
8. 编译 PDF，检查图表编号、交叉引用、目录和浮动体位置。

最终目标不是让论文“少代码”，而是让读者先理解系统机制，再把代码作为关键证据。系统论文可以有工程细节，但工程细节必须服务于抽象贡献。
