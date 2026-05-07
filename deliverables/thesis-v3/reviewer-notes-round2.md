# 二审修改建议

审阅对象：`deliverables/thesis-v3/main.pdf`

审阅时间：2026-05-07

本轮重点复核上一轮问题的修改情况，包括 A2A 新颖性表述、实验图表一致性、baseline 论证与构建日志。总体来看，A2A 主张已经大面积降级，这是正确方向；但仍有几处旧口径残留和数据同步问题，需要提交前处理。

## 总体结论

当前稿件已经从“容易被 A2A 新颖性打穿”变成“工程论文可评审”。主要贡献定位更稳，实验章也补充了 baseline 缺失的局限说明。

但仍未达到最终提交态，剩余问题集中在四类：

1. 第 4 章仍残留旧版 A2A 生态表述。
2. 第 5 章对内部 baseline 的说法仍偏推断。
3. event_flow p50 数字未全文同步。
4. 构建门禁仍未覆盖 missing character 等渲染问题。

## P1：必须修

### 1. 第 4 章 A2A 官方 SDK 现状仍写错

位置：

- `deliverables/thesis-v3/chapters/04-implementation.tex:330`

当前问题：

该处仍写：

> A2A 在 Rust 生态没有任何官方或第三方实现——Google 仅发布了 Python 与 Go 参考实现……

这已经与当前 A2A Project 官方页面冲突。A2A Project 官方 README 已列出 `a2a-rs` 作为 Official Rust SDK；同时生态中也已有若干第三方 Rust A2A crate。该句如果保留，会直接破坏相关工作综述与贡献定位的可信度。

建议改法：

将这一段改成与第 1 章、第 3 章一致的口径：

> A2A（Agent-to-Agent）是 Google 在 2025 年提出并由 Linux Foundation 托管的开放 agent 互通协议。当前 A2A 生态已包含官方与第三方多语言 SDK，其中 Rust 生态已有 `a2a-rs` 等实现。本文没有把贡献建立在“Rust 生态空白”之上，而是面向 Fuxi 的本地优先编排场景，从零实现一份与事件总线、玄女—门客编排层、反向 WebSocket sandbox 同源演进的 A2A 核心闭环，并在状态机层扩展 `InputRequired` 人工介入语义。

需要同步检查：

- `04-implementation.tex` 中所有“官方仅 Python/Go”“无第三方实现”“完整 A2A 实现”类似表述。
- 摘要、绪论、第三章、结论目前大体已改，但仍建议全文 `rg "尚不存在|没有任何|官方仅|首个|唯一|完整 A2A"` 再扫一遍。

### 2. 第 4 章小结残留旧版新颖性主张

位置：

- `deliverables/thesis-v3/chapters/04-implementation.tex:687`

当前问题：

该处仍写：

> fuxi-a2a 从零实现了 Rust 生态尚不存在的 A2A 1.0 完整客户端与服务端……

这与前文已修正后的贡献定位冲突。论文前面已经承认 Rust 生态中存在 `a2a-rs`、`a2a-client`、`a2a-types` 等实现，因此此处必须同步。

建议改法：

> 看到 fuxi-a2a 面向 Fuxi 本地优先 Agent 协作场景实现了 A2A 核心 RPC 闭环，并通过 `InputRequired` 状态扩展了 A2A 规范以支持人在回路场景……

或更完整：

> 看到 fuxi-a2a 并非以通用 SDK 为目标，而是把 A2A wire、JSON-RPC、SSE 流与 Fuxi 的事件总线、玄女—门客编排层和反向 WebSocket sandbox 结合起来，形成面向本地优先协作场景的协议实现。

## P2：建议修

### 3. 内部 baseline 论证仍偏推断

位置：

- `deliverables/thesis-v3/chapters/05-experiments.tex:205`

当前问题：

该段把 `poll_ms` 扫描和 `event_flow` 延迟称为：

> 这两组数据给出了一个最小内部 baseline……

但实际并没有直接实现并测量“事件驱动 push dispatch 替代 worker poll”的版本。当前只是将：

- dispatch 路径的 poll-only 行为；
- observation 路径的 EventBus 延迟；

放在同一机器上进行间接比较。这可以作为内部参照，但不应称为 baseline。

建议改法：

将“baseline”降级为“内部参照”：

> 这两组数据提供了一个内部参照：当前 dispatch 路径采用 poll-only 模型，在 sustained throughput 场景下未表现为主要瓶颈；而 observation 路径的 EventBus 通知延迟已处于亚毫秒量级。需要注意的是，本文并未实现并实测 push-based dispatch，因此“事件驱动派发能否降低 burst 场景延迟”仍属于后续工作。

建议补一句限制：

> 因此，本节只能说明当前 poll-only dispatch 在 sustained 负载下可接受，不能证明 push-based dispatch 在所有场景下收益有限。

### 4. event_flow p50 数字未全文同步

位置：

- `deliverables/thesis-v3/chapters/05-experiments.tex:131`
- `deliverables/thesis-v3/chapters/05-experiments.tex:267`
- `deliverables/thesis-v3/chapters/05-experiments.tex:275`
- 摘要、结论中若有相同数字也需同步检查

当前问题：

表 5.3 已写：

```text
event_flow p50 = 0.08 ms
event_flow p99 = 0.26 ms
```

但正文仍有多处：

```text
event_flow p50 = 0.07 ms
L_evt = 0.07 ms
```

虽然差异只有 0.01 ms，但上一轮已经指出图表一致性问题，这类残留会让审稿人怀疑数据链路不严。

建议：

- 全文统一使用 `0.08 ms`。
- 第 5.8 节端到端延迟分解中同步改为 `$L_{evt}=0.08$ ms`。
- 若图 5.6 是由脚本自动生成，也要重新生成图，避免图中仍显示 0.07。
- 全文搜索：

```bash
rg "0\\.07|event_flow p50|L_\\{evt\\}|Levt" deliverables/thesis-v3
```

## 构建与渲染问题

### 5. 构建门禁仍未覆盖 missing character

位置：

- `deliverables/thesis-v3/build.sh`
- `deliverables/thesis-v3/build.log`

当前情况：

构建脚本已经比上一轮更好，末轮会检查：

- fatal error；
- undefined reference；
- undefined citation。

但末轮日志仍出现：

```text
Missing character: There is no → (U+2192) in font Helvetica Neue/AAT:mapping=tex-text;!
Missing character: There is no → (U+2192) in font Helvetica Neue/AAT:mapping=tex-text;!
```

这类问题不会导致编译失败，但会造成 PDF 中对应字符缺失。最终提交前建议把以下内容也纳入门禁：

- `Missing character`
- `multiply-defined labels`
- `Overfull \hbox` 中明显超出版心的项

建议脚本增加：

```bash
MISSING_CHAR=$(awk -v n="$LASTRUN" 'NR>=n' build.log | grep -c "Missing character" || true)
MULTI_LABEL=$(awk -v n="$LASTRUN" 'NR>=n' build.log | grep -c "multiply-defined labels" || true)
```

并在门禁条件中加入：

```bash
if [ "$ERRORS" -gt 0 ] || [ "$UNDEF_REF" -gt 0 ] || [ "$UNDEF_CITE" -gt 0 ] || [ "$MISSING_CHAR" -gt 0 ] || [ "$MULTI_LABEL" -gt 0 ]; then
  exit 1
fi
```

对于当前 `→` 缺字问题，可以考虑：

- 把正文中的 `→` 改成 LaTeX 数学箭头 `$\to$`；
- 或改成中文“到/指向/进入”；
- 或保证该字符使用支持箭头的字体。

## 已改善项

### A2A 新颖性主张

多数章节已经从“Rust 生态首个/唯一”改成了“与 Fuxi 平台同源演进”，方向正确。

已较稳的表达包括：

- “与 fuxi 平台同源演进的 A2A v1.0 实现”
- “与事件总线、玄女—门客编排层、反向 WebSocket sandbox 深度集成”
- “InputRequired 人工介入语义的协议级扩展”
- “差异化贡献不在协议覆盖度本身”

这些表述可以保留。

### 横向 baseline 缺失

第 6 章已经将横向 baseline 对比缺失列入局限，方向正确。第 5 章也补充了理论上限对照的理由。剩余问题只是不要把未实测的 push dispatch 对照称为 baseline。

### 图表数据

表 5.3 与摘要、结论中的 p99 已基本同步到：

- `task_dispatch p99 = 36.66 ms`
- `event_flow p99 = 0.26 ms`

剩余主要是 p50 的 `0.07` / `0.08` 小数残留。

## 推荐收尾顺序

1. 改 `04-implementation.tex:330`，删掉“Rust 生态没有任何官方或第三方实现”。
2. 改 `04-implementation.tex:687`，删掉“Rust 生态尚不存在”。
3. 改 `05-experiments.tex:205`，把“最小内部 baseline”降级为“内部参照”，并明确 push dispatch 未实测。
4. 全文统一 `event_flow p50` 为 `0.08 ms`，同步第 5.8 节和图 5.6。
5. 处理 `Missing character: →`，并把 missing character 纳入构建门禁。
6. 重新编译 PDF，确认末轮日志无 undefined、无 duplicate label、无 missing character。

## 最终判断

完成上述修改后，这篇论文的主要审稿风险会明显下降。A2A 贡献不再依赖易被外部事实推翻的“首个/唯一”主张，而是落在更稳的工程集成与协议扩展上；实验章也能以“理论上限对照 + 内部参照 + 明确局限”的方式自洽。
