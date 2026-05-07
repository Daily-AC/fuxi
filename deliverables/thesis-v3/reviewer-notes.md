# 审稿视角修改建议

审阅对象：`deliverables/thesis-v3/main.pdf`

审阅范围：PDF 正文、TeX 源文件、主要图表页、构建日志、参考文献记录。

## 总体结论

当前稿件的工程内容扎实，系统设计、实现细节和实验解释都明显强于一般工程型论文。主要风险不在“内容不够”，而在以下三点：

1. 核心新颖性主张过满，尤其是 A2A Rust 生态“首个/唯一/尚无 SDK”的表述已经不稳。
2. 实验章缺少最小内部 baseline，部分论证依赖类比而非本系统消融。
3. 构建与图表一致性还没达到提交状态。

建议按本文最后的优先级列表逐项处理。

## P0：必须修改

### 1. 降级 A2A Rust 生态“首个/唯一”主张

相关位置：

- `frontmatter/abstract-cn.tex`
- `chapters/01-introduction.tex`
- `chapters/03-modules.tex`
- `chapters/04-implementation.tex`
- `chapters/06-conclusion.tex`

当前稿中多处使用了类似表述：

- “A2A 协议在 Rust 生态中尚无可用 SDK”
- “Rust 生态首个完整 A2A v1.0 实现”
- “当前 Rust 生态唯一一个完整的 A2A 1.0 实现”
- “Rust 生态中无任何官方或社区的完整实现”

这些表述风险很高。当前公开资料显示：

- A2A Project GitHub 已列出 Official Rust SDK `a2a-rs`。
- `a2a-rust` 文档声称提供 A2A v1.0.0 Rust implementation。
- docs.rs 上已有 `xa2a` Rust A2A SDK。

建议改写方向：

- 从“生态首个/唯一”改成“面向 Fuxi 场景的轻量 Rust A2A 实现”。
- 从“填补 Rust 生态空白”改成“在本地优先、多进程门客、事件总线集成场景下实现 A2A 核心闭环”。
- 把贡献重点转向：
  - 与 Fuxi 事件总线/编排层的集成；
  - `InputRequired` 人工介入语义；
  - 单端点 HTTP+SSE 的工程取舍；
  - 小规模、可审阅、可测试的协议子集实现。

示例改法：

> 本文实现了一个面向 Fuxi 本地优先多 Agent 场景的轻量 Rust A2A 核心闭环，覆盖 `agent/getCard`、`tasks/send`、`tasks/sendSubscribe`、`tasks/get` 与 `tasks/cancel` 等关键方法，并在状态机层面扩展 `InputRequired` 以表达人工介入语义。

### 2. 修复构建成功判定与重复 label

相关位置：

- `build.sh`
- `build.log`
- `chapters/03-modules.tex`
- `chapters/04-implementation.tex`

当前构建脚本吞掉了 `xelatex` 非零退出：

```bash
xelatex -interaction=nonstopmode main.tex >>build.log 2>&1 || true
```

这会让“PDF 生成”掩盖真实 LaTeX 错误。构建日志末尾仍有：

```text
LaTeX Warning: There were multiply-defined labels.
```

已确认重复 label：

- `chapters/03-modules.tex` 中 `lst:eventbus-publish`
- `chapters/04-implementation.tex` 中 `lst:eventbus-publish`

建议：

- 两处代码片段保留一个完整 listing，另一处改成引用或改不同 label。
- 构建脚本不要吞 `xelatex` 错误。
- 提交前目标：
  - 末轮 0 个 undefined citation；
  - 末轮 0 个 undefined reference；
  - 0 个 multiply-defined label；
  - 重要 overfull hbox 清零或可解释。

### 3. 统一表格与图中的实验数据

相关位置：

- `chapters/05-experiments.tex`
- `figures/matplotlib/plot_5_4_dispatch_latency.py`
- `figures/matplotlib/plot_5_5_event_flow_latency.py`
- `benchmarks/latency-samples.csv`

目前存在图表数据不一致：

- 表 5.3：`task_dispatch p99 = 37.32 ms`
- 图 5.1：标注 `p99 = 36.5 ms`
- 表 5.3：`event_flow p99 = 0.15 ms`
- 图 5.2：标注 `p99 = 216.1 us`

差异不大，但审稿人会怀疑表和图不是同一批数据。

建议：

- 表格和图统一从同一个 CSV 或同一个 markdown benchmark 文件生成。
- 若图表确实来自不同 run，应在图注或正文明确说明，但更推荐统一数据源。
- 重新生成 PDF 后再人工检查图中标注、正文数字、摘要数字是否一致。

## P1：强烈建议修改

### 4. 补一个最小内部 baseline

相关位置：

- `chapters/05-experiments.tex`

当前稿解释了为什么不引入外部系统 baseline，这个理由基本成立。但事件驱动是本文核心设计之一，仅靠 Kafka、ZooKeeper、Etcd 的经验类比来说明“优于轮询”，说服力不足。

建议补一个最小内部消融 baseline，成本不需要很高：

- 方案 A：关闭事件驱动通知，仅使用 poll 兜底。
- 方案 B：保留事件通知，但关闭 WAL 写入，测实时路径上限。
- 方案 C：比较 Tokio broadcast fan-out 与 per-worker mpsc fan-out。

最低可接受实验：

- 固定 `worker_n=4`
- 固定 `job_sleep=10 ms`
- 固定 `tasks_n=400`
- 扫 `poll_ms = {5, 10, 25, 50, 100}`
- 比较“事件通知 + poll 兜底”与“纯 poll”

这样可以直接支撑“真实时不轮询”这个设计公理，而不是只靠外部系统引用。

### 5. 收敛实验章的防御性论证

相关位置：

- `chapters/05-experiments.tex`

当前“为什么不做 baseline”的说明有价值，但写法略显防御。建议保留核心理由，压缩语气，并把缺失横向 baseline 明确列为局限。

建议表述方向：

> 本章选择理论上限作为主要参照，以隔离语言运行时和框架语义差异对结果的干扰。横向对比同类 Agent 编排系统需要统一负载、统一任务语义与统一观测口径，本文未覆盖该部分，后续工作将补充。

### 6. 重新审视“完整 A2A 兼容”的边界

相关位置：

- `chapters/04-implementation.tex`
- `chapters/06-conclusion.tex`

当前稿一方面说“完整 A2A v1.0 实现”，另一方面又明确简化掉 push notification、双向 TLS、自动重连、版本协商。审稿人可能质疑“完整”的定义。

建议：

- 改为“A2A 核心 RPC 闭环”或“A2A v1.0 核心子集”。
- 明确“完整覆盖本文系统所需的 agent discovery、task send、task stream、task query、task cancel 闭环”。
- 不要把未实现项描述成完整实现的一部分。

## P2：建议修改

### 7. 删除或替换图 1.1

相关位置：

- `chapters/01-introduction.tex`
- `figures/gpt-image/fig-1-1-overview.png`

图 1.1 信息密度偏低，更像概念装饰图，与图 2.1 的真实架构重复。建议二选一：

- 直接删除图 1.1，让第 1 章更像论文而非项目介绍页。
- 替换为真实运行时概览图，至少包含：
  - 玄女；
  - 门客；
  - EventBus；
  - SQLite WAL；
  - IM/TUI/PWA；
  - A2A endpoint；
  - 事件流与控制流。

### 8. 放大或简化图 2.2

相关位置：

- `chapters/02-overall-design.tex`
- `figures/drawio/fig-2-2-dataflow.pdf`

图 2.2 的时序关系是有价值的，但打印版上右侧第 6/7 步、虚线说明和小字标签略吃力。

建议：

- 宽度调到 `0.98\textwidth`；
- 减少灰色辅助线；
- 只保留关键箭头说明，把细节放进正文。

### 9. 重画或改造图 5.5

相关位置：

- `chapters/05-experiments.tex`
- `figures/matplotlib/plot_5_3_bus_stress.py`

图 5.5 右图混用了 log 轴和 `<1 us` 类型数据，视觉上不够直观。建议：

- 把 `<1 us` 统一设为 0.5 us 仅用于绘图，并在图注说明；
- 或者只画出现拐点的 64 sub × 100k 场景；
- 或者表格为主，图只展示 drops 与 p99 的趋势。

### 10. 压缩工程博客式语气

相关位置：全文，尤其是第 4 章和第 5 章。

建议替换的表达类型：

- “亏也亏不到哪里去”
- “小动作的纪律”
- “普通本科生 2 周左右的全职工作量”
- “所有人都可以直接 cargo add”
- “朴素够用就别搞工业级”

这些句子有工程现场感，但在论文里容易显得主观。建议改成更克制的论文语气：

- “内存开销可忽略”
- “实现复杂度较低”
- “代码规模为 1321 行”
- “可作为 Rust A2A agent 集成的轻量实现”
- “在当前需求下未引入额外复杂机制”

## 其他观察

### 优点

- 论文有清晰的工程主线：本地优先、多进程门客、事件总线、A2A、SQLite WAL、可观测性。
- 第 5 章的方法学说明较充分，warmup、噪声控制、线程数固定、时钟精度等内容能提升可信度。
- 图 2.1、图 2.3 的控制流/事件流区分较清楚，适合作为系统设计图。
- 第 4 章代码级解释充分，读者能从实现细节看到系统真实存在，而不是停留在概念设计。
- “局限与不足”章节方向正确，已经承认跨节点、安全模型、真实 LLM 负载等问题。

### 剩余风险

- 论文标题写“分布式通讯系统”，但跨节点实验较少，主要实验仍是单机。需要在摘要、实验章和结论中明确“本文稳定性承诺建立在单机/本地优先部署上，跨节点能力为扩展验证”。
- A2A、MCP、Claude Code、Codex CLI 等对象变化很快，提交前需要重新核对引用与生态现状。
- PDF 抽文本时出现字体 mismatch warning，虽然不一定影响提交，但最好在最终构建环境中确认字体嵌入和学校系统上传预览效果。

## 推荐修改顺序

1. 全文替换 A2A Rust 生态“首个/唯一/尚无”的表述。
2. 修复重复 label 和构建脚本吞错问题。
3. 统一实验图表数据，重新生成图和 PDF。
4. 补一个最小内部 baseline；如果来不及补实验，则把无 baseline 明确写进局限。
5. 删除或替换图 1.1，调整图 2.2 和图 5.5。
6. 全文扫一遍主观、口语、自夸式表达。
7. 最后跑一次干净构建，并人工检查摘要、目录、图表、参考文献和 PDF 预览。
