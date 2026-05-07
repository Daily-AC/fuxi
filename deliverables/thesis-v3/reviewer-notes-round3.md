# 三审审稿意见

审阅对象：`deliverables/thesis-v3/main.pdf`

审阅依据：最新提交 `5326f0b fix(thesis): round-2 review 5 项必修全部落地`

审阅时间：2026-05-07

本轮以顶刊审稿人的标准复核上一轮修改是否真正收敛，重点不再重复评价全篇工程内容，而是检查贡献定位、相关工作口径、实验数字和构建门禁是否仍存在会影响可信度的残留问题。

## Part 1：Review Report

### Summary

本文提出并实现了一个本地优先、事件驱动、可观测、可跨节点扩展的 Rust 多 Agent 协作平台 Fuxi，将 A2A 协议闭环、事件总线、玄女—门客编排、反向 WebSocket sandbox 与跨节点扩展钩子整合为一个可运行的工程系统。

### Strengths

1. **系统贡献已经比较清晰。** 修订后论文不再把贡献主要建立在“Rust 生态首个 A2A SDK”这类高风险外部事实之上，而是转向“面向 Fuxi 本地优先协作场景的协议闭环与平台集成”。这个定位更稳，也更符合论文实际完成的工程工作。

2. **实验章的防御面明显收窄。** 第 5 章已经把 `poll_ms` 扫描和 `event_flow` 延迟降级为“内部参照”，并明确说明本文没有实现和实测 push-based dispatch。这个修改避免了把间接比较包装成消融 baseline 的问题。

3. **数据链路和构建门禁比上一轮更接近提交态。** `event_flow p50` 已统一为 `0.08 ms`，图 5.6 已重新生成；`build.sh` 也开始检查 `Missing character` 与重复 label，说明作者已经把“能生成 PDF”和“可提交 PDF”区分开来。

### Weaknesses (Critical)

#### W1. 引言仍残留 A2A 官方 SDK 旧口径，影响相关工作可信度

位置：

- `deliverables/thesis-v3/chapters/01-introduction.tex:40`
- `deliverables/thesis-v3/chapters/01-introduction.tex:56`

问题：

引言仍写“A2A 协议官方仅维护 Python 与 Go 实现”“其官方仅维护 Go 与 Python 实现”。这与第 4 章修订后的新口径冲突：第 4 章已经承认当前 A2A 生态中已有官方与第三方 SDK，Rust 生态已有 `a2a-rs`、`a2a-client`、`a2a-types` 等实现。公开 A2A SDK 页面也已列出 Rust SDK `a2a-rs`。

这不是单纯措辞问题，而是会直接影响审稿人对“相关工作是否查清楚”的判断。引言是读者第一次接触论文贡献边界的位置；如果这里仍保留“官方仅 Go/Python”的过期表述，即使第 4 章已经修正，读者也会认为论文内部口径不一致，甚至怀疑作者是在被动规避前文的新颖性风险。

建议改法：

将第 40 行改成：

> A2A 生态已出现官方与第三方多语言 SDK，Rust 生态内已有 `a2a-rs`、`a2a-client`、`a2a-types` 等实现。本文的贡献不在于填补 Rust SDK 空白，而在于面向 Fuxi 的本地优先编排场景，实现与事件总线、玄女—门客编排层和反向 WebSocket sandbox 深度集成的 A2A 核心闭环，并扩展 `InputRequired` 人工介入语义。

将第 56 行改成：

> Agent2Agent 协议是 Google 主导、Linux Foundation 维护的 Agent 间协作开放标准。当前生态中已有官方与第三方 SDK；本文选择实现 `fuxi-a2a`，原因不在协议结构本身，而在于需要把 A2A 与 Fuxi 的事件总线、玄女—门客编排层和反向 WebSocket sandbox 三者深度耦合。

#### W2. 本章小结仍使用“A2A v1.0 Rust 从零实现”，新颖性叙事尚未完全收敛

位置：

- `deliverables/thesis-v3/chapters/01-introduction.tex:86`

问题：

本章小结把贡献一概括为“A2A v1.0 Rust 从零实现”。这个说法虽然已经弱于“首个”“唯一”“尚不存在”，但仍容易把读者带回“Rust 生态缺失，因此本文从零实现”的旧叙事。考虑到论文已经承认 Rust 生态中存在相关 SDK 和 crate，此处应避免把“从零实现”作为一级贡献标签。

建议改法：

将：

> A2A v1.0 Rust 从零实现

改为：

> 面向 Fuxi 本地优先协作场景的 A2A v1.0 核心闭环实现

或：

> 与 Fuxi 平台同源演进的 A2A 协议适配层实现

这样可以把贡献锚定在“平台语义集成”和“工程闭环”上，而不是锚定在“Rust 语言实现的新颖性”上。

### Rating

**预估评分：8 / 10。**

理由：论文的工程系统完整度、实现透明度和实验解释已经达到较强工程型系统论文的水准；当前剩余问题主要是引言贡献口径残留，不是方法或实验层面的结构性缺陷。若上述两处修完，稿件可以进入接近提交态。

## Part 2：Strategic Advice

### 问题根源

当前问题的根源不是系统工作本身不成立，而是论文在多轮修订后留下了“旧贡献叙事”和“新贡献叙事”的局部重叠。

旧叙事是：

> Rust 生态缺少 A2A，因此本文从零实现一个完整 A2A SDK。

新叙事是：

> A2A 生态已有官方与第三方实现，本文贡献在于面向 Fuxi 本地优先多 Agent 协作场景，把 A2A wire、JSON-RPC/SSE、事件总线、编排层、反向 WebSocket sandbox 与 `InputRequired` 语义集成起来，形成可运行、可观测、可测试的平台闭环。

第二种叙事明显更强，也更不容易被外部事实击穿。现在第 3、4、5 章基本已经转向新叙事，但第 1 章仍残留旧叙事，因此审稿人会感到“作者知道这个风险，但没有全文收敛干净”。

### 可救性判断

这两个问题都属于**可在修订期内完全解决的表述一致性问题**，不是方法缺陷，也不需要补实验。

不需要新增代码，不需要新增图表，也不需要重跑实验。需要做的是：

1. 修改第 1 章相关工作段落，使其承认 A2A 官方与第三方 SDK 的当前状态。
2. 修改贡献一段落，明确 `fuxi-a2a` 的实现动机是平台深度集成，不是通用 SDK 竞争。
3. 修改本章小结，把“A2A v1.0 Rust 从零实现”改为“面向 Fuxi 场景的 A2A 核心闭环实现”。

### 行动指南

建议按以下顺序收尾：

1. 全文搜索并替换高风险词：

```bash
rg "官方仅|仅维护|从零实现|首个|唯一|尚不存在|没有任何|完整 A2A|Rust 生态空白" deliverables/thesis-v3
```

2. 对每个命中项按新口径判断：

- 可以保留：“本文从零实现 `fuxi-a2a` crate”，前提是它只说明项目内部实现方式。
- 不应保留：“Rust 生态从零实现”“官方仅 Go/Python”“Rust 生态尚不存在”。
- 建议替换：“核心闭环实现”“协议适配层实现”“面向 Fuxi 场景的深度集成实现”。

3. 顺手统一摘要口径。

摘要中“Rust 生态中虽已有若干第三方 crate”不是硬伤，但建议改成“已有若干官方与第三方 SDK/crate”。摘要是审稿人最先读的部分，不应让读者误以为论文仍在弱化 `a2a-rs` 的官方状态。

4. 修改后重新构建并检查门禁。

```bash
cd deliverables/thesis-v3
./build.sh
rg "Missing character|multiply-defined|undefined references|undefined citations" build.log
```

目标状态：

- 末轮无 undefined reference。
- 末轮无 undefined citation。
- 无 `Missing character`。
- 无重复 label。
- PDF 中第 1 章、第 4 章、摘要对 A2A 生态的表述一致。

## 三审结论

本轮没有发现新的实验硬伤，也没有发现会推翻系统贡献的结构性问题。上一轮指出的第 4 章、baseline、p50 同步和构建门禁问题已经基本修复。

当前唯一必须处理的是第 1 章残留的 A2A 官方 SDK 旧口径。这个问题小，但权重高，因为它位于引言和贡献定义处，会影响审稿人对全文可信度的第一判断。修完后，论文的攻击面将主要退回到“横向 baseline 有限”和“跨节点验证规模较小”这类作者已承认的局限，而不再是事实性表述错误。

参考核验：

- A2A SDK 页面：<https://a2a-protocol.org/latest/sdk/>
- A2A Project GitHub：<https://github.com/a2aproject>
