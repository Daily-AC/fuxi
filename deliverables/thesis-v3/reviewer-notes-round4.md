# 四审审稿意见

审阅对象：`deliverables/thesis-v3/main.pdf`

审阅依据：最新提交 `e767f0e fix(thesis): round-3 review · A2A 官方 SDK 现状 + 贡献一新颖性叙事彻底收敛`

审阅时间：2026-05-07

本轮按顶刊系统论文审稿人的标准复核最新版。三审指出的“官方仅 Go/Python”和“A2A v1.0 Rust 从零实现”旧口径已经基本修正；但继续核对 A2A 官方当前规范后，发现一个更深的规范版本问题：论文多处仍把当前实现描述为“A2A v1.0 兼容”并把 `InputRequired` 描述为 Fuxi 对 A2A v1.0 的扩展，这与当前官方 A2A v1.0/最新规范不一致。

## Part 1：Review Report

### Summary

本文实现了一个本地优先、事件驱动、可观测、可跨节点扩展的 Rust 多 Agent 协作平台 Fuxi；其工程贡献主要在于把事件总线、编排层、反向 WebSocket sandbox、A2A 风格协议闭环和跨节点扩展钩子整合成一个可运行系统。

### Strengths

1. **贡献定位比三审前明显稳。** 论文现在已经承认 Rust 生态存在官方 `a2a-rs` 与第三方 crate，不再把贡献建立在“Rust 生态空白”上。这是正确方向。

2. **实验章基本进入可评审状态。** `event_flow` 数字同步、baseline 降级、端到端拆分注明估算性质、构建门禁收紧，这些修改有效降低了实验可信度风险。

3. **系统论文的工程主线仍然成立。** EventBus、WAL、玄女—门客、WS 反连、sandbox、跨节点 trait 注入这些内容构成了一个完整系统，而不是单点功能堆叠。若 A2A 规范版本问题处理干净，论文的主要可辩护贡献仍然扎实。

### Weaknesses (Critical)

#### W1. A2A v1.0 兼容性主张与当前官方规范冲突

严重级别：P0

相关位置：

- `deliverables/thesis-v3/frontmatter/abstract-cn.tex:12`
- `deliverables/thesis-v3/frontmatter/abstract-en.tex:11`
- `deliverables/thesis-v3/chapters/01-introduction.tex:56`
- `deliverables/thesis-v3/chapters/02-overall-design.tex:150`
- `deliverables/thesis-v3/chapters/03-modules.tex:66`
- `deliverables/thesis-v3/chapters/03-modules.tex:93`
- `deliverables/thesis-v3/chapters/04-implementation.tex:334`
- `deliverables/thesis-v3/chapters/04-implementation.tex:405`
- `deliverables/thesis-v3/chapters/04-implementation.tex:622`
- `deliverables/thesis-v3/chapters/06-conclusion.tex:14`

问题：

论文当前仍反复使用“A2A v1.0 兼容”“任何符合 A2A v1.0 的客户端都能与伏羲服务器互通”“InputRequired 是伏羲对 A2A 1.0 的实质性扩展”等表述。但当前 A2A 官方规范中，`TaskState` 已包含 `TASK_STATE_INPUT_REQUIRED`、`TASK_STATE_REJECTED` 与 `TASK_STATE_AUTH_REQUIRED`；`InputRequired` 不是 Fuxi 新增的规范扩展。官方规范还把 JSON-RPC 方法命名列为 PascalCase，例如 `SendMessage`、`SendStreamingMessage`、`GetTask`、`CancelTask`、`SubscribeToTask`；而论文和代码描述的是 `agent/getCard`、`tasks/send`、`tasks/sendSubscribe`、`tasks/get`、`tasks/cancel` 这一组 legacy / draft 风格方法名。官方当前 `Part` 模型也采用 one-of 字段语义，即 `text`、`raw`、`url`、`data` 四者必须恰有一个存在；论文第 3 章却把 `#[serde(tag = "type")]` 的 tagged union 作为 wire 兼容设计来论证。

这会被顶刊审稿人视为规范理解错误，而不是普通措辞问题。当前稿件一方面引用 `a2aproject2025a2a` 并把访问日期写到 2026-05-07，另一方面使用的 wire 形态、方法名与状态机叙事更像早期 A2A draft / legacy JSON-RPC 子集。若不处理，审稿人会质疑：

- 论文实现到底兼容哪个 A2A 版本？
- `InputRequired` 到底是规范已有语义、Fuxi 的内部状态映射，还是作者自创扩展？
- “任意 A2A v1.0 兼容方互通”是否经过官方 SDK 互操作测试？
- 若方法名和 `Part` JSON 形态不一致，为什么还能称为 A2A v1.0 compatible wire protocol？

建议修法有两条，必须二选一。

方案 A：保守且适合论文收尾。

把全文“A2A v1.0 兼容”降级为：

> 面向 Fuxi 场景的 A2A 风格核心 RPC 闭环实现

或：

> 基于早期 A2A JSON-RPC 形态的本地优先协议适配层

同时将 `InputRequired` 的贡献从“协议扩展”改为：

> 将 A2A 规范已有的 `input-required` / interrupted task 语义映射到 Fuxi 的 `Task::PendingApproval` 与 `ShelfStatus::AwaitingInput`，并把人工介入从内部状态提升为平台级可观测事件。

这样保留工程贡献，但放弃“规范级新增”的高风险叙事。

方案 B：激进且需要更多工程工作。

将 `fuxi-a2a` 更新到当前官方 v1.0/最新规范：

- JSON-RPC 方法改为 `SendMessage`、`SendStreamingMessage`、`GetTask`、`CancelTask`、`SubscribeToTask` 等官方命名。
- `TaskState` 对齐官方状态集合，至少补齐 `Rejected`、`AuthRequired`，并把 `InputRequired` 标为规范已有 interrupted state。
- `Part` wire 形态从 tagged union 改为官方 one-of 字段语义。
- 增加与官方 `a2a-rs` 或 Python SDK 的互通测试。

如果不能在提交前完成方案 B，就采用方案 A。不要继续写“A2A v1.0 兼容”。

#### W2. 第 4 章小结仍把 `a2a-rs` 归入第三方 crate

严重级别：P1

位置：

- `deliverables/thesis-v3/chapters/04-implementation.tex:626`

问题：

该处写：

> 与生态内 a2a-rs、a2a-client、a2a-types 等第三方 crate 处在不同定位上

但前文已经改成 `a2a-rs` 是 A2A 项目官方 Rust SDK，不能再把它和 `a2a-client`、`a2a-types` 一起称为“第三方 crate”。这属于三审问题的局部残留，虽然比 W1 小，但位置在第 4 章总结段，容易再次制造“作者没有全文收敛”的印象。

建议改为：

> 与生态内官方 SDK `a2a-rs` 以及 `a2a-client`、`a2a-types` 等第三方 crate 处在不同定位上：前者以通用 SDK 或协议封装为目标，本 crate 则面向 Fuxi 平台内部的事件总线、编排层、反向 WebSocket sandbox 与人工介入状态流转做深度集成。

#### W3. “任何 A2A v1.0 客户端互通”的外延过大，当前证据不足

严重级别：P1

相关位置：

- `deliverables/thesis-v3/chapters/03-modules.tex:66`
- `deliverables/thesis-v3/chapters/04-implementation.tex:334`
- `deliverables/thesis-v3/chapters/04-implementation.tex:626`

问题：

论文声称“任何符合 A2A v1.0 的客户端都能与伏羲服务器互通，反之亦然”，以及“证明本实现的协议合规性达到了规范级别”。但目前证据只是：

- Fuxi 自己的 roundtrip 测试；
- cc/codex 适配器；
- 一个外部 Python 参考实现。

这些证据可以证明“核心路径在作者控制的 endpoint 上可用”，不能证明“任意 A2A v1.0 客户端双向互通”。尤其在 W1 指出的规范版本漂移存在时，这句话会被审稿人优先攻击。

建议改为：

> 这些测试证明 `fuxi-a2a` 在 Fuxi 所需的 discovery、send、stream、get、cancel 五条核心路径上形成了稳定闭环，并与作者维护的一个 Python 参考 endpoint 完成互通；与当前官方 SDK 的完整互操作验证留待后续工作。

如果后续真的补官方 SDK 互通测试，再把结论升回“与官方 SDK 兼容”。

### Rating

**当前预估评分：7 / 10。**

理由：系统实现和实验本身已经有较强说服力，但 A2A 规范版本与兼容性主张存在 P0 级事实风险。这个问题不修，审稿人会从“相关工作查得不够细”升级为“核心协议贡献定义不清”。若采用保守改法把 A2A 表述降级并全文一致，评分可回到 8 / 10 附近。

## Part 2：Strategic Advice

### 问题根源

三审解决的是“生态里有没有 Rust SDK”的问题；四审暴露的是“你实现的到底是不是当前 A2A v1.0”的问题。

这两个问题层级不同。前者只是贡献定位过满，降级为平台深度集成即可；后者涉及协议 wire 形态、状态机、方法命名和互操作边界。如果论文继续用当前官方规范作为引用对象，就必须接受官方当前规范对 `TaskState`、`Part`、JSON-RPC 方法名和版本协商的定义。不能一边引用当前规范，一边沿用早期 draft 的 wire 形态，再称为 v1.0 compatible。

### 可救性判断

W1 是严重问题，但不是不可救。

最务实的修订路径不是改代码，而是改论文口径。因为本文主要贡献不依赖“完全兼容官方 A2A v1.0”。真正有价值的是：

- Fuxi 把 agent 间通讯做成平台级协议闭环；
- `input-required` 这类人工介入语义被映射到内部调度状态与事件流；
- A2A 风格的 send / stream / get / cancel 核心路径与 EventBus、WS sandbox、玄女—门客编排同源演进；
- 实现足够小，测试覆盖足够集中，可支撑本地优先平台使用。

因此建议不要硬辩“兼容 v1.0”。把贡献重写为“面向 Fuxi 场景的 A2A 风格核心协议适配层”，论文会更稳。

### 行动指南

#### 1. 全文替换高风险词

建议先跑：

```bash
rg "A2A v1\\.0 兼容|A2A 1\\.0 兼容|任何符合 A2A|任意 A2A|双向兼容|规范级别|协议合规|InputRequired.*扩展|新增.*InputRequired|原规范只有 5|五态|5 态|第三方 crate" deliverables/thesis-v3/frontmatter deliverables/thesis-v3/chapters
```

替换原则：

- “A2A v1.0 兼容”改为“A2A 风格核心闭环”或“早期 A2A JSON-RPC 子集”。
- “InputRequired 协议扩展”改为“将 A2A 已有人工介入语义映射到 Fuxi 编排状态与事件流”。
- “任何 A2A v1.0 客户端互通”改为“在本文覆盖的五条核心路径上与作者维护的 endpoint 互通”。
- “规范级别合规”改为“核心路径行为稳定，官方 SDK 互通留待后续工作”。

#### 2. 建议改写贡献一

当前贡献一的推荐新口径：

> 贡献一：与 Fuxi 平台同源演进的 A2A 风格协议适配层。本文实现了覆盖 agent discovery、task send、task stream、task query 与 task cancel 五条核心路径的轻量协议闭环。该实现并不以替代官方 `a2a-rs` 或完整覆盖当前 A2A v1.0 规范为目标，而是面向 Fuxi 本地优先协作场景，将 A2A 的任务、消息、artifact 与 interrupted task 语义映射到事件总线、玄女—门客编排层和反向 WebSocket sandbox 中，使人工介入、状态回放和多端观测成为平台级能力。

这个版本没有事实性风险，且更贴近本文真实贡献。

#### 3. 第 2、3、4、6 章同步收敛

重点改以下句子：

- `02-overall-design.tex:40`：`A2A InputRequired 状态扩展` 改为 `A2A input-required 语义的平台映射`。
- `02-overall-design.tex:150`：删除“A2A v1.0 原规范只有 5 态终态机”和“关键扩展”。
- `03-modules.tex:66`：删除“任何符合 A2A v1.0 的客户端都能互通”。
- `03-modules.tex:93`：把方法名解释为“Fuxi 当前采用的 legacy / 内部 A2A-style 方法名”，不要说这是当前 v1.0 官方方法集。
- `04-implementation.tex:334`：删除“足以与任意 A2A 1.0 兼容方互通”。
- `04-implementation.tex:622`：删除“伏羲对 A2A 1.0 的实质性扩展”。
- `06-conclusion.tex:14`：删除“新增 InputRequired”和“协议级扩展”。

#### 4. 可选补一条局限说明

在第 6 章局限中加入一条：

> A2A 规范在 2025--2026 年间仍处于快速演进阶段。本文的 `fuxi-a2a` 实现覆盖了 Fuxi 所需的核心协作闭环，但尚未与当前官方 `a2a-rs` SDK 做完整 wire-level 互操作验证，也未完全覆盖 v1.0 最新规范中的全部方法、状态与 binding 细节。后续工作将把该 crate 升级为当前官方规范兼容层，或显式维护 legacy binding 适配器。

这条局限会显著降低攻击面。

### 构建与图表状态

PDF 已更新到 90 页，生成时间为 2026-05-07 22:43:02。末轮构建日志未看到新的 `Missing character` 或重复 label；`build.log` 中仍有早期 pass 的 undefined reference / citation 警告，但 `build.sh` 当前只统计最后一轮，这是合理的。

本轮抽查了摘要页、引言页和实验图表页。版式没有发现新的提交级阻断问题；主要风险已经不是图表，而是 A2A 规范表述。

## 四审结论

三审问题已经修掉，但论文现在暴露出一个更根本的协议版本问题：当前实现可以被描述为“Fuxi 场景下的 A2A 风格核心闭环”，但不能在不补官方 v1.0 wire-level 互通测试的情况下继续称为“当前 A2A v1.0 兼容实现”，也不能把 `InputRequired` 说成 Fuxi 对 A2A v1.0 的新增扩展。

建议立刻按保守方案收敛口径。这样做不会削弱论文的真实贡献，反而会让贡献从“和官方规范争定义”回到“把 agent 协作平台做成可运行系统”这一更强、更稳的主线上。

参考核验：

- A2A 官方 SDK 页面：<https://a2a-protocol.org/latest/sdk/>
- A2A 官方规范：<https://a2a-protocol.org/latest/specification/>
- 官方规范中 `TaskState` 已包含 `TASK_STATE_INPUT_REQUIRED`，见 §4.1.3。
- 官方规范中 `Part` 使用 one-of 字段语义，见 §4.1.6。
- 官方规范中 JSON-RPC 方法采用 `SendMessage`、`SendStreamingMessage`、`GetTask` 等命名，见 §9.1 与 §9.4。
