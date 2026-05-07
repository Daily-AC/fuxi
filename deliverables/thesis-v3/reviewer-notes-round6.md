# 六审审稿意见

审阅对象：`deliverables/thesis-v3/main.pdf`

审阅依据：最新提交 `9bcd6fe fix(thesis): round-5 review · 一级贡献标题与 4 处规范映射收尾`

审阅时间：2026-05-07

本轮继续以顶刊系统论文审稿人口径复核。五审指出的四处旧口径已经基本处理：第 1 章贡献标题已降级为“A2A 风格协议适配层”，`InputRequired` 不再作为 Fuxi 对规范的新增扩展，`agent/getCard` 也不再直接等同于 `GetExtendedAgentCard`。论文主线现在已经稳定回到“本地优先多 Agent 协作平台”的系统贡献上。

当前没有 P0。剩余问题是 A2A 当前规范中几个状态枚举细节仍有小错，建议提交前清理。

## Part 1：Review Report

### Summary

本文实现了 Fuxi，一个本地优先、事件驱动、可观测、可跨节点扩展的 Rust 多 Agent 协作平台；最新版已将 A2A 贡献准确降级为“沿用早期 A2A JSON-RPC binding 的平台级协议适配层”，系统贡献边界基本清楚。

### Strengths

1. **A2A 主张已经基本收敛。** 最新稿不再主张当前官方 A2A v1.0 兼容，也不再把 `InputRequired` 作为 Fuxi 对规范的新扩展。第 1、3、4、6 章现在都承认早期 binding、当前官方规范与官方 SDK 互通待补，这是正确的防御姿态。

2. **贡献标题和摘要已经可提交。** 中文摘要、英文摘要和第 1 章贡献一都改成“A2A 风格核心 RPC 闭环 / 协议适配层”，避免了“首个 SDK”“完整 v1.0 实现”等高风险说法。

3. **构建状态仍稳定。** 最新 PDF 为 91 页，生成时间为 2026-05-07 23:17:49。末轮构建日志未见 `Missing character`、重复 label 或末轮 undefined reference/citation。剩余 Underfull hbox 和字体替代不构成审稿层面的阻断。

### Weaknesses (Critical)

#### W1. `Rejected` 被误归为 interrupted / non-terminal state

严重级别：P1

相关位置：

- `deliverables/thesis-v3/chapters/02-overall-design.tex:150`
- `deliverables/thesis-v3/chapters/03-modules.tex:70`
- `deliverables/thesis-v3/chapters/04-implementation.tex:359`

问题：

最新版多处写：

> `InputRequired`、`AuthRequired`、`Rejected` 三个 interrupted/non-terminal 状态

或：

> 与 `auth-required`、`rejected` 同属规范状态集中的非终态分支

这与当前 A2A 官方规范不一致。官方规范中：

- `TASK_STATE_INPUT_REQUIRED` 是 interrupted state；
- `TASK_STATE_AUTH_REQUIRED` 是 interrupted state；
- `TASK_STATE_REJECTED` 是 terminal state。

也就是说，`Rejected` 可以和 `Completed`、`Failed`、`Canceled` 一样归入终态集合，不能和 `InputRequired` / `AuthRequired` 一起说成 non-terminal 或 interrupted 分支。

建议改法：

`02-overall-design.tex:150`：

> 这是 A2A 规范已有的 interrupted task state；当前规范还包含 `auth-required` 这一授权中断态，以及 `rejected` 这一拒绝执行终态。

`03-modules.tex:70`：

> A2A 当前规范定义的 `TaskState` 状态集合包括 `Submitted`、`Working`、`Completed`、`Failed`、`Canceled` 五个核心态，另有 `InputRequired` 与 `AuthRequired` 两个 interrupted state，以及 `Rejected` 这一拒绝执行终态。

`04-implementation.tex:359`：

> 当前官方规范已正式纳入 `InputRequired` 与 `AuthRequired` 两个 interrupted state，并包含 `Rejected` 这一终态。

#### W2. 当前规范枚举值与早期 binding 字符串仍混用

严重级别：P2

相关位置：

- `deliverables/thesis-v3/chapters/02-overall-design.tex:150`
- `deliverables/thesis-v3/chapters/03-modules.tex:70`
- `deliverables/thesis-v3/chapters/04-implementation.tex:359`
- `deliverables/thesis-v3/chapters/04-implementation.tex:618`

问题：

论文现在已经明确 `fuxi-a2a` 沿用早期 binding，并在 wire 上使用 `"input-required"`。这可以保留。但当正文说“当前 A2A 官方规范已有 input-required”时，最好区分：

- 当前官方规范 / ProtoJSON 枚举值：`TASK_STATE_INPUT_REQUIRED`；
- Fuxi 早期 binding wire 字符串：`"input-required"`。

当前稿在一些地方直接把 `state: input-required` 与当前官方规范绑定在一起。官方规范当前写法是 `TASK_STATE_INPUT_REQUIRED`，并说明 JSON enum 使用 ProtoJSON 的 string name。虽然读者能理解你在说同一语义，但顶刊审稿人如果继续按规范逐字核对，会认为这里仍不够精确。

建议统一口径：

> A2A 当前规范中对应语义为 `TASK_STATE_INPUT_REQUIRED`；Fuxi 沿用的早期 JSON-RPC binding 在 wire 上编码为 `"input-required"`。

局部替换示例：

> 当门客遇到歧义需要人工答复时，Fuxi 通过早期 binding 发出 `state: "input-required"`；该语义对应当前 A2A 规范中的 `TASK_STATE_INPUT_REQUIRED` interrupted state。

这样就不会把 legacy wire 字符串误写成当前官方规范的 JSON 枚举值。

#### W3. “任何兼容客户端识别”仍略微过满

严重级别：P2

位置：

- `deliverables/thesis-v3/chapters/03-modules.tex:87`

问题：

这里写：

> 显式状态枚举则可被任何兼容客户端识别

在论文已经承认 `fuxi-a2a` 未与当前官方 `a2a-rs` 做 wire-level 互通验证、且沿用早期 binding 的前提下，“任何兼容客户端”外延仍然偏大。更稳的说法是“支持该状态语义的客户端”或“理解该 binding 的客户端”。

建议改为：

> 显式状态枚举则可被支持该状态语义的客户端识别；这一点也正是 A2A 当前官方规范将 `TASK_STATE_INPUT_REQUIRED` 纳入正式 `TaskState` 的原因。

这保留论证，不再引入过宽互通承诺。

### Rating

**当前预估评分：8.2 / 10。**

理由：现在已经没有会推翻贡献定位的事实性硬伤，系统实现和实验都处于可评审状态。剩余问题是 A2A 状态枚举的精确性和少数措辞外延，属于提交前打磨项。

## Part 2：Strategic Advice

### 问题根源

五审之后，主要叙事已经收敛；六审看到的是“为了修 A2A 版本问题而补进的新解释里，又混入了少量当前规范细节错误”。

这很常见：当论文从“我实现了 v1.0”退到“我沿用早期 binding，但映射当前规范语义”时，需要同时维护两套名词：

1. 当前官方规范名词：`TASK_STATE_INPUT_REQUIRED`、`TASK_STATE_AUTH_REQUIRED`、`TASK_STATE_REJECTED`；
2. Fuxi 早期 binding 名词：`"input-required"`、`"auth-required"`、`"rejected"`。

只要把“规范语义”和“Fuxi wire 字符串”明确分开，问题就解决了。

### 可救性判断

这三个问题都是**文本精度问题**，不需要改代码，不需要补实验，不影响论文主贡献。

其中 W1 建议必须修，因为它是明确事实错误；W2 和 W3 是降低攻击面的措辞修正。

### 行动指南

建议直接搜索：

```bash
rg "interrupted/non-terminal|非终态分支|state: input-required|任何兼容客户端|auth-required.*rejected|InputRequired.*AuthRequired.*Rejected" deliverables/thesis-v3/chapters
```

替换原则：

- `Rejected` 一律写成终态，不再和 `InputRequired` / `AuthRequired` 一起归为 interrupted。
- 当前官方规范枚举值写 `TASK_STATE_INPUT_REQUIRED` / `TASK_STATE_AUTH_REQUIRED` / `TASK_STATE_REJECTED`。
- Fuxi 早期 binding wire 字符串写 `"input-required"` / `"auth-required"` / `"rejected"`。
- “任何兼容客户端”改为“支持该状态语义的客户端”。

修完后再跑：

```bash
cd deliverables/thesis-v3
./build.sh
rg "interrupted/non-terminal|非终态分支|任何兼容客户端" chapters
```

目标是无命中。

## 六审结论

六审没有发现新的 P0，也没有发现实验或构建层面的阻断问题。当前稿件已经非常接近提交态。

最后需要修的只是 A2A 状态集合的精确表述：`InputRequired` 和 `AuthRequired` 是 interrupted state，`Rejected` 是 terminal state；当前官方枚举名和 Fuxi 早期 binding 字符串要分开写。修掉这些后，A2A 这条攻击面基本就收干净了。

参考核验：

- A2A 官方规范：<https://a2a-protocol.org/latest/specification/>
- 官方规范 §4.1.3：`TASK_STATE_INPUT_REQUIRED` 为 interrupted state，`TASK_STATE_AUTH_REQUIRED` 为 interrupted state，`TASK_STATE_REJECTED` 为 terminal state。
- 官方规范 §5.5：JSON enum values 使用 ProtoJSON string name，例如 `"TASK_STATE_INPUT_REQUIRED"`。
