# 五审审稿意见

审阅对象：`deliverables/thesis-v3/main.pdf`

审阅依据：最新提交 `8266f31 fix(thesis): round-4 review · A2A 规范版本主张降级 + InputRequired 不再称扩展`

审阅时间：2026-05-07

本轮继续以顶刊系统论文审稿人口径复核。四审的 P0 风险已经大幅收敛：论文不再主张当前 A2A v1.0 完整兼容，已明确 `fuxi-a2a` 沿用早期 A2A JSON-RPC binding，并把与官方 `a2a-rs` 的 wire-level 互通列入局限。这是正确方向。

剩余问题不再是“会打穿全文”的结构性硬伤，而是若干旧词残留和局部规范映射不精确。它们建议提交前处理，但不需要再补实验。

## Part 1：Review Report

### Summary

本文实现了一个本地优先、事件驱动、可观测、可跨节点扩展的 Rust 多 Agent 协作平台 Fuxi；最新版本已把 A2A 贡献从“当前官方 v1.0 兼容 SDK”降级为“面向 Fuxi 场景的 A2A 风格协议适配层”，贡献边界比上一轮更清晰。

### Strengths

1. **四审 P0 基本修复。** 第 3、4、6 章已经明确写出：`fuxi-a2a` 使用早期 A2A binding，方法名与当前官方 PascalCase 方法集、`Part` one-of 语义并不完全对齐，官方 SDK 互通留待后续工作。这解决了上一轮最危险的“规范版本错配”问题。

2. **InputRequired 叙事已经转向工程映射。** 第 2、4、6 章现在把 `input-required` 描述为 A2A 规范已有 interrupted state，并强调 Fuxi 的贡献是把它映射到 `Task::PendingApproval` / `ShelfStatus::AwaitingInput` 与事件流，而不是为 A2A 新增状态。这一口径更可信。

3. **构建与版式未见新的阻断问题。** 最新 PDF 为 91 页，生成时间为 2026-05-07 23:02:49。末轮构建日志未见 `Missing character`、重复 label 或末轮 undefined reference/citation；当前剩余多为 Underfull hbox 和中文斜体字体替代，不构成提交阻断。

### Weaknesses (Critical)

#### W1. 第 1 章仍残留“A2A v1.0 实现 / 扩展 InputRequired”的旧贡献标题

严重级别：P1

相关位置：

- `deliverables/thesis-v3/chapters/01-introduction.tex:40`
- `deliverables/thesis-v3/chapters/01-introduction.tex:56`

问题：

第 1 章主体段落已经在同一段末尾说明“本文不主张 v1.0 兼容”，但标题和切入句仍写：

> 贡献一：与 fuxi 平台同源演进的 A2A v1.0 实现

以及：

> 与本地优先编排层和事件总线深度集成、并扩展 InputRequired 人工介入语义

这两处会让读者在进入贡献列表时再次回到旧叙事：仿佛本文仍在主张“A2A v1.0 实现”以及“扩展 InputRequired”。虽然第 56 行后半段已经解释不主张 v1.0 兼容，但标题是审稿人最容易记住的部分，应当比正文更保守。

建议改法：

将第 40 行的切入句改为：

> 本工作不依托「Rust 生态空白」立论，而是面向「与本地优先编排层和事件总线深度集成、并将 A2A 已有 `input-required` 语义映射为平台级人工介入状态」这一具体场景，实现一份与 fuxi 平台同源演进的 A2A 风格协议适配层。

将第 56 行标题改为：

> 贡献一：与 fuxi 平台同源演进的 A2A 风格协议适配层

不要再在一级贡献标题里使用“A2A v1.0 实现”。如果要保留 v1.0，只能写成“参照 A2A v1.0 任务语义的核心闭环”，但这仍不如“A2A 风格协议适配层”稳。

#### W2. 第 3 章仍用“这一扩展”指代 InputRequired，局部语义残留

严重级别：P2

位置：

- `deliverables/thesis-v3/chapters/03-modules.tex:85`

问题：

这里写：

> 这一扩展的工程价值在于把人工介入从「编排器内部隐式状态」提升为「协议级显式状态」。

在四审后，`InputRequired` 已经不能再被称为 Fuxi 对 A2A 的扩展；后文第 87 行虽然解释“官方规范选择把 InputRequired 纳入正式 TaskState”，但第 85 行的“这一扩展”仍会被审稿人标出来。

建议改为：

> 这一平台映射的工程价值在于把人工介入从「编排器内部隐式状态」提升为「可被协议字段承载、可被事件流观测的显式状态」。

这样既保留工程贡献，也避开“规范扩展”的歧义。

#### W3. 第 4 章把 legacy `agent/getCard` 对应到 `GetExtendedAgentCard`，映射过于武断

严重级别：P2

位置：

- `deliverables/thesis-v3/chapters/04-implementation.tex:405`

问题：

该处写：

> `agent/getCard` ... 对应当前规范的 `GetExtendedAgentCard`

当前 A2A 官方规范里 `GetExtendedAgentCard` 是“retrieves an extended Agent Card”，并且 extended Agent Card access control 要求鉴权；而 `agent/getCard` 在早期 binding 中更像公开 agent discovery / card 获取路径。二者不能直接等号对应。这里若写“对应”，审稿人可能追问：Fuxi 的 `agent/getCard` 是否实现 extended card 鉴权语义？是否支持 public card 与 authenticated extended card 的差异？如果没有，就不应把它映射为 `GetExtendedAgentCard`。

建议改为：

> 所有方法沿用早期 A2A binding 命名（`agent/getCard`、`tasks/send`、`tasks/get`、`tasks/cancel`、`tasks/sendSubscribe`）。其中 task send / get / cancel / stream 在语义上分别接近当前规范的 `SendMessage`、`GetTask`、`CancelTask`、`SendStreamingMessage` / `SubscribeToTask` 路径；agent card 获取路径则属于早期 discovery binding，未实现当前规范中 authenticated extended card 的完整语义。

这比直接“对应 GetExtendedAgentCard”更准确。

#### W4. 第 4 章仍写“Google 规范允许两种实现风格”，证据不足

严重级别：P2

位置：

- `deliverables/thesis-v3/chapters/04-implementation.tex:524`

问题：

该处写：

> Google 规范允许两种实现风格："单端点 + method 分发 + SSE 升级"和"双端点 + 显式 `/a2a/stream`"

在当前官方规范中，JSON-RPC binding 明确描述 HTTP(S) + JSON-RPC + SSE，并列出 `SendStreamingMessage` 与 `SubscribeToTask` 的 SSE 响应形式；但“允许两种实现风格”这个表述需要非常具体的规范依据。当前论文没有给出这一依据，且前文已经承认 Fuxi 沿用早期 binding，因此更稳的写法是把它改成 Fuxi 的工程选择，而不是官方规范许可。

建议改为：

> 伏羲选择保留早期 binding 中的单端点 method 分发与 SSE 升级形式，因为这能简化 reverse proxy 配置，nginx 只需转发一个 location；当前官方 binding 的完整端点与方法形态不在本工作覆盖范围内。

这样不会把一个工程取舍说成规范事实。

### Rating

**当前预估评分：8 / 10。**

理由：上一轮 P0 已基本处理，系统贡献、实验和局限说明都已经进入可评审状态。剩余问题主要是第 1 章标题与少数实现段落的术语残留，不再影响系统方法本身。若处理 W1-W4，稿件可进入接近最终提交态。

## Part 2：Strategic Advice

### 问题根源

当前残留的根源是“正文解释已经改成保守口径，但标题、转折句和旧段落中的高权重词还没完全同步”。

顶刊审稿人读论文时会优先抓三类位置：

1. 摘要和贡献标题；
2. 相关工作中的差异化定位；
3. 实现章节的小结和规范映射句。

现在第 3、4、6 章的大部分正文已经能自洽，但第 1 章贡献标题仍写“A2A v1.0 实现”，第 3 章仍有“这一扩展”，第 4 章仍有“对应当前规范”和“规范允许两种实现风格”。这些词本身不长，但会把审稿人的注意力重新拉回 A2A 规范争议。

### 可救性判断

这些问题都属于**提交前文本收敛问题**，不是方法缺陷，不需要改代码，不需要补实验。

最重要的是不要再继续解释“为什么其实也能算”。现在最稳的策略是统一降级：

- 不说“v1.0 实现”，说“A2A 风格协议适配层”。
- 不说“InputRequired 扩展”，说“input-required 语义的平台映射”。
- 不说“对应当前规范”，说“语义上接近 / 早期 binding 中的核心路径”。
- 不说“规范允许”，说“Fuxi 选择 / 本工作采用”。

### 行动指南

建议直接按以下搜索收尾：

```bash
rg "A2A v1\\.0 实现|扩展 InputRequired|这一扩展|对应当前规范|Google 规范允许|任何兼容客户端|协议级扩展|规范级别" deliverables/thesis-v3/frontmatter deliverables/thesis-v3/chapters
```

逐项替换为：

- `A2A v1.0 实现` -> `A2A 风格协议适配层`
- `扩展 InputRequired` -> `映射 A2A 已有 input-required 语义`
- `这一扩展` -> `这一平台映射`
- `对应当前规范` -> `语义上接近当前规范中的...`
- `Google 规范允许` -> `Fuxi 选择`

修完后再构建一次：

```bash
cd deliverables/thesis-v3
./build.sh
```

构建通过后建议再跑：

```bash
rg "Missing character|multiply-defined|undefined references|undefined citations" build.log
rg "A2A v1\\.0 实现|扩展 InputRequired|这一扩展|对应当前规范|Google 规范允许" chapters frontmatter
```

目标状态是：第一条只命中早期 pass 的正常引用收敛信息或无命中；第二条无命中。

## 五审结论

五审结论比四审乐观：协议版本 P0 已经被控制住，论文主线回到了“本地优先多 Agent 协作平台”而不是“当前 A2A 官方 SDK 竞争者”。当前最需要做的是把第 1 章贡献标题和少数规范映射句改干净。

如果 W1-W4 处理完，后续再审大概率不会再有事实性硬伤，只会剩下“横向 baseline 有限”“跨节点规模较小”“官方 A2A SDK 互通待补”这些已经在局限中承认的正常攻击面。

参考核验：

- A2A 官方 SDK 页面：<https://a2a-protocol.org/latest/sdk/>
- A2A 官方规范：<https://a2a-protocol.org/latest/specification/>
- 官方规范中 `TaskState` 包含 `TASK_STATE_INPUT_REQUIRED`、`TASK_STATE_REJECTED`、`TASK_STATE_AUTH_REQUIRED`，见 §4.1.3。
- 官方规范中 `Part` 使用 one-of 字段语义，见 §4.1.6。
- 官方规范中 JSON-RPC binding 要求 PascalCase 方法命名，并列出 `SendMessage`、`SendStreamingMessage`、`GetTask`、`CancelTask`、`SubscribeToTask`，见 §9.1 与 §9.4。
