# Fuxi 毕设论文 design-spec

题目：基于 AI Agent 的高性能分布式通讯系统
作者：以琳（湖南第一师范学院）
日期：2026-05-07
本文目的：把 brainstorming 阶段所有决策固化，给后续 writing-plans 与执行 agent 当唯一真相源。

> 旧的 v0 草稿已归档到 `deliverables/thesis-v0-archived-2026-05-07/`，下游一律以本目录 `deliverables/thesis-v2/` 为准。

---

## 1. 总体定位

**写作目标**：本科毕业论文（湖南第一师范学院理科模板），但**内容质量向顶刊看齐**——问题驱动、形式化定义、真实可复现的实验、可被搜索到的参考文献。

**外壳**：6 章 + 摘要 + 目录 + 参考文献 + 致谢，符合学校模板结构。
**内核**：每章按"问题 → 方案 → 验证 → 小结"的顶刊逻辑组织。

**总篇幅**：正文 26000 中文字 ± 10%，外加摘要、目录、参考文献、致谢约 4000 字。

**项目背书**：论文以仓库 `/Users/e0_7/fuxi`（fuxi 个人 AI agent 平台）为研究对象，所有代码引用、benchmark 数据、模块名称均与代码一致。

---

## 2. 章节框架与字数预算

| 章 | 标题 | 字数 | 核心交付 |
|---|---|---|---|
| 1 | 绪论 | 4500 | 背景、国内外现状、研究内容、论文结构；refs 密度最高（约 18 篇集中此章） |
| 2 | 系统总体设计 | 4000 | 需求、整体架构、通信模型形式化、性能指标定义；公式 (2-1)~(2-3) + 1 张架构总图 |
| 3 | 核心模块设计 | 6000 | 7 个模块（通信协议/事件总线/编排/工作区/观测/记忆/触发器）；公式 (3-1)~(3-2) + 7 张图 |
| 4 | 系统实现 | 4500 | Rust workspace、数据结构、派发流程、分布式实现；1 张 dist 流图 + 4 段伪代码 |
| 5 | 实验与分析 | 5500 | 测试方法、吞吐、延迟、poll_ms 扫描、WAL 对比、scalability 曲线；6 张实验图 + 4 张表 |
| 6 | 总结与展望 | 1500 | 总结、不足、后续 |
| **正文合计** | | **26000** | |

附加：摘要 ~400 字 / Abstract ~250 词 / 关键词 5 个 / 目录 / 30 篇参考文献 / 致谢 ~200 字。

每章结构统一为：
- 章首引言段（150-200 字）：本章定位 + 与上一章的衔接
- 中部 N 个二级标题章节（按上表分配）
- 章末小结（200-300 字）：本章要点 + 引出下一章

---

## 3. 学校格式硬约束（来自《格式规范注意事项》+ 理科模板）

### 3.1 文档结构（顺序固定）

1. 封面（题目、姓名、学号、班级、专业、指导教师、年月）
2. 扉页（同封面 + 完成日期）
3. 诚信声明（电子签名 + 答辩之后日期）
4. 中文摘要 + 关键词
5. 英文摘要（ABSTRACT） + Key words
6. 目录
7. 正文 1-N 章（标题用 `1`、`1.1`、`1.1.1`，**无"第 N 章"前缀**）
8. 参考文献（英文文献 ≥ 2 篇）
9. 致谢
10. 附录（可选）

封面、扉页、摘要、目录**不编页码**。正文起开始编页码。

### 3.2 字体字号（手调清单，markdown 写不出来，留给用户）

| 元素 | 字体 | 字号 |
|---|---|---|
| 一级标题 | 黑体 | 小三 |
| 二级标题 | 黑体 | 四号 |
| 三级标题 | 黑体 | 小四 |
| 正文 | 宋体 | 小四 |
| 数字与字母 | Times New Roman | 同正文 |
| 页眉 | 黑体 | 小五 |
| 页码 | 宋体 | 小五 |

**正文行距 1.5 倍，字符间距标准。**

页边距：上 2.5 / 下 2 / 左 3 / 右 2 cm（左边距宽是装订侧）。

### 3.3 编号规范

- 公式：`(章号-序号)`，如 `(2-1)`、`(3-2)`
- 图：`图章号.序号`，如 `图2.3`，标题居中在图下
- 表：`表章号.序号`，如 `表5.2`，标题居中在表上
- 参考文献：`[1]`，英文一行写不下行尾加 `-`

### 3.4 标题与查重一致

封面、扉页、页眉、查重报告四处的论文题目必须**逐字一致**。论文题目锁定为：
**基于 AI Agent 的高性能分布式通讯系统**
（注意是"通讯"不是"通信"，与你给的题目保持一致；摘要、关键词等内文里可以视语境用"通信"，但题目和页眉锁定"通讯"）

---

## 4. 参考文献策略

### 4.1 总数与桶分布（30 篇）

| 桶 | 数量 | 候选清单（按引用顺序） |
|---|---|---|
| A. 多智能体理论基础 | 4 | Wooldridge MAS 教材；Stone & Veloso 多智能体综述；Russell & Norvig AI 教材；Jennings 关于自治 agent 的早期论文 |
| B. LLM Agent 综述与代表系统 | 8 | Wang et al. LLM Agent Survey；Yao et al. ReAct；Schick Toolformer；Wu et al. AutoGen；Hong et al. MetaGPT；Li CAMEL；Wang Voyager；Wang OpenHands |
| C. Agent 通信协议与编排 | 5 | Anthropic MCP；A2A 官方 spec；Karpas MRKL；LangGraph (Harrison Chase et al.)；Significant-Gravitas AutoGPT |
| D. 分布式系统经典 | 7 | Lamport 1978 Time-Clocks-Order；Dean & Ghemawat MapReduce；Ghemawat GFS；Kreps Kafka；Ongaro & Ousterhout Raft；Hunt et al. ZooKeeper；Corbett et al. Spanner |
| E. 工程实现/工具 | 6 | Tokio 文档；SQLite WAL 文档；Axum 文档；git-worktree 文档；RFC 6455 WebSocket；HTML5 Server-Sent Events spec |

**每桶不严格定数**——若 A 找不到 4 篇高质量，可以用 B/C 顶上，最终 ≥ 25 即可。

### 4.2 验证流程（硬要求：每篇都必须 search 得到）

每引用一篇前，必须完成：
1. WebSearch + arxiv API 查到论文标题、作者、年份
2. 校对 BibTeX 字段（author / title / venue / year / arxiv id）
3. 把验证证据（搜索结果片段或 URL）写到 `refs-verification.md`
4. 失败的立即从候选清单换别的

最终交付 `refs.bib`（可被 pandoc-citeproc 引用）+ `refs-verification.md`（可审计的证据链）。

### 4.3 国内中文文献

学校允许英文文献 ≥ 2 即可，**不要求**中文文献。但为了多样性，可以挑 2-3 篇国内系统综述（如《软件学报》、《计算机学报》上的多智能体或分布式系统综述）。仍按上述验证流程走。

---

## 5. 数学公式清单

公式不堆——每个都为正文论证服务。

| 编号 | 公式 | 用处 |
|---|---|---|
| (2-1) | `L_e2e = L_enq + L_disp + L_exec + L_evt` | 端到端延迟分解，2.4 通信模型 |
| (2-2) | `T = N / W_total` | 吞吐定义，2.4 |
| (2-3) | `η = 1 - T_actual / T_ideal` | 调度损耗，2.4，给第 5 章测吞吐铺垫 |
| (3-1) | `L_evt ≥ max_i (T_serialize + T_channel + T_consume_i)` | 广播事件延迟下界，3.3 事件总线 |
| (3-2) | `select(t) = argmin_w {load(w) | tags(t) ⊆ tags(w), pin(t) ∈ {∅, w.node}}` | Worker 选择函数，3.4 编排调度 |
| (4-1) | `C(a→b) = C(a) + 1; C(receive(m)) = max(C_local, C(m)) + 1` | Lamport 事件因果序，4.3 事件数据结构 |

数字写法：行内公式用 `$...$`，独立公式用 `$$...$$` + 编号。中文标点用全角，公式内用半角。

---

## 6. 图表清单与工具分工

### 6.1 draw.io 架构/流程图（8 张）

| 编号 | 标题 | 内容 |
|---|---|---|
| 图2.1 | Fuxi 系统总体架构 | 5 层（核心类型 / 通信 / 编排 / 执行 / 观测）+ 各 crate 归属 |
| 图3.1 | 模块依赖关系图 | crate 之间的 use 依赖 + 主要 trait 边界 |
| 图3.2 | 任务生命周期状态机 | Created → Dispatched → Running → Done/Cancelled/Failed |
| 图3.3 | 事件总线发布订阅拓扑 | publisher → broadcast channel + SQLite WAL → N 个 subscriber |
| 图3.4 | 工作区隔离三层 | L1 read-only / L2 ephemeral / L3 persistent + git worktree |
| 图3.5 | 玄女门客协作时序 | 用户 → 玄女 → dispatch → 门客 → 事件回流 → 玄女抄送 |
| 图4.1 | 跨节点 dist 任务流 | home → dist controller → 远端 worker → event 回传 |
| 图4.2 | Agent adapter 与 CLI 进程边界 | Rust trait → spawn → CLI stdout 解析 → EventKind |

### 6.2 matplotlib 实验图（6 张）

| 编号 | 标题 | 数据来源 |
|---|---|---|
| 图5.1 | 吞吐与 worker 数线性扩展 | scalability 实验 |
| 图5.2 | poll_ms 参数消融 | poll_ms scan 实验 |
| 图5.3 | 事件总线 publish 吞吐 vs subscriber 数 | 事件总线纯压测 |
| 图5.4 | 任务派发延迟分布（小提琴图）| latency 实验 500 样本 |
| 图5.5 | 跨节点事件流延迟分布 | latency 实验 |
| 图5.6 | 端到端延迟分解柱状图 | 把 (2-1) 各分量实测拆出来 |

样式约定：A4 单栏宽度（~6.5 inch），dpi=300，字号 11pt，色盲友好（用 `viridis` 或 `tab10`）。

### 6.3 gpt-image-2 概念图（1-2 张）

| 编号 | 用处 | 内容 |
|---|---|---|
| 图1.1 | 第 1 章引子 | 玄女与门客的拟人化协作场景（中式插画风，符合"玄女/门客"文化命名） |
| 图6.1（可选）| 第 6 章展望 | 多节点分布式 fuxi 群的概念示意 |

> 概念图仅作章首/章末点缀，不进入论证链路。学校老师对插画接受度未知，若觉得不严肃可以砍。

### 6.4 表格清单（6 张）

| 编号 | 标题 | 位置 |
|---|---|---|
| 表4.1 | Fuxi workspace crate 与职责 | 4.2 |
| 表5.1 | 系统测试覆盖情况 | 5.2 |
| 表5.2 | 分布式任务吞吐量测试结果 | 5.3 |
| 表5.3 | 任务派发与事件流延迟 | 5.4 |
| 表5.4 | poll_ms 扫描结果 | 5.5 |
| 表5.5 | 事件总线纯压测吞吐与延迟 | 5.6 |

---

## 7. 实验补做计划（A 路线）

### 7.1 重跑 baseline（取 5-run median）

- 执行：`cargo bench -p fuxi-cli --bench run_baseline`
- 输出：覆盖 `docs/benchmarks/baseline-2026-05-07.md`
- 估时：~5 min × 5 = 25 min

### 7.2 poll_ms 扫描

- 设计：`poll_ms ∈ {5, 10, 25, 50, 100}` × `job_sleep ∈ {10ms, 100ms, 1000ms}` × `worker_n=4`
- 实现：扩展 `crates/fuxi-cli/src/bench_support/throughput.rs` 加一组 cell；环境变量传 poll_ms
- 输出：`docs/benchmarks/poll-scan-2026-05-07.md`
- 估时：~30 min 跑 + 1h 画图分析

### 7.3 事件总线纯压测（替代原 WAL on/off）

- **动机**：现有 e2e baseline 把通信层性能埋在 agent dispatch 开销下；论文题目"高性能分布式通讯"必须**单独**测通信层。同时为公式 (3-1) 给的延迟下界提供实测兑现。
- 设计：
  - 单 publisher 直调 `EventBus::publish`，N 个 subscriber 各自 `subscribe()` 接收
  - 维度 1：N ∈ {1, 4, 16, 64} subscriber，看延迟 + 吞吐随订阅者数的衰减
  - 维度 2：发送速率从 1k/s 提到 100k/s，看 broadcast 何时开始丢帧（`RecvError::Lagged`）
  - 维度 3：事件 payload size ∈ {小（< 256B EventKind 简单变体）, 大（含 4KB AssistantText）}
- 实现：新建 `crates/fuxi-events/benches/bus_stress.rs`（与现有 dist_bench_common 解耦，纯进程内 channel 测试，不走 HTTP）
- 输出：`docs/benchmarks/eventbus-stress-2026-05-07.md`
- 估时：~1h 写 bench + 30 min 跑

WAL 设计动机在 §3.3 用 1-2 段散文交代即可，不另开实验。

### 7.4 Scalability 1/2/4/8/16 worker

- 设计：固定 `job_sleep=10ms`、`tasks_per_worker=100`、`poll_ms=5`，扫 worker_n
- 实现：扩展 `throughput.rs`
- 输出：`docs/benchmarks/scalability-2026-05-07.md`
- 估时：~30 min

**总数据汇总**：`docs/benchmarks/v2-2026-05-07.md`（论文 cite 这一份）。

---

## 8. 工作流与并行策略

### 8.1 三路并行

用 superpowers `TeamCreate` + `Agent` 起一个 thesis 团队：

- **draft 路**：起草章节 markdown（按章号顺序，1→6）
- **refs 路**：30 篇参考文献验证 + bib 整理（独立于章节起草）
- **bench 路**：4 项实验 + 出图 + 写 benchmarks v2 报告

draft 路对 refs 路有软依赖（写到引用时拿 bib 中已验证的）；draft 路第 5 章对 bench 路硬依赖（5 章必须等实验完）。

### 8.2 单章流程

每章草稿出来后：
1. **润色**：`awesome-ai-research-writing/表达润色（中文论文）` prompt
2. **去 AI 味**：`awesome-ai-research-writing/去 AI 味（Word 中文）` prompt
3. 自查：本章字数、引用闭合、图表编号、公式编号

全文齐备后：
4. **Reviewer 自审**：`awesome-ai-research-writing/论文整体以 Reviewer 视角进行审视`，用真审稿口径检查

### 8.3 文件布局

```
deliverables/thesis-v2/
├── design-spec.md            # 本文档（设计真相源）
├── main.md                   # 完整论文（pandoc 入口）
├── chapters/                 # 各章独立 markdown，main.md 用 include 拼起来
│   ├── 00-frontmatter.md     # 封面/扉页/诚信声明/摘要/目录
│   ├── 01-introduction.md
│   ├── 02-overall-design.md
│   ├── 03-modules.md
│   ├── 04-implementation.md
│   ├── 05-experiments.md
│   ├── 06-conclusion.md
│   └── 99-acknowledgements.md
├── refs.bib                  # 验证后的 BibTeX
├── refs-verification.md      # 每篇 search 证据
├── figures/
│   ├── drawio/               # 8 张 .drawio + 导出 .png
│   ├── matplotlib/           # 6 张 .py + .png
│   └── gpt-image/            # 1-2 张 .png
├── benchmarks/
│   ├── v2-2026-05-07.md      # 总报告
│   ├── poll-scan/            # 子项原始数据
│   ├── wal-compare/
│   └── scalability/
├── format-checklist.md       # 给用户的 docx 格式手调清单
└── pandoc.sh                 # main.md → docx 导出脚本
```

---

## 9. 时间线（感性，不是 DDL）

按 CLAUDE.md 公理 7：毕设不是 DDL，是顺带。下面只是参考节奏：

- 今晚（2026-05-07）：spec approved → writing-plans → 起 team → bench 路开跑、refs 路开始验证、draft 路开始 1/2 章
- 明天：3/4 章 + 实验数据齐
- 后天：5/6 章 + 摘要 + 全文润色
- 随后：用户 review + 改 + 导 docx + 学校格式手调

---

## 10. 风险与对策

| 风险 | 概率 | 对策 |
|---|---|---|
| 找不到 25 篇都通过 search 验证 | 中 | 桶分布留 30 篇缓冲；中文期刊也接受；实在不够就找学位论文（CNKI）补 |
| 事件总线压测出现 broadcast lag 频繁丢帧 | 中 | lag 本身就是发现——把"系统在 X events/s 开始丢帧"当结果而不是 bug |
| 第 5 章实验图风格不一致 | 中 | 所有 matplotlib 图共用一个 `style.py`，统一 rc params |
| draft 与 refs 路时序错位（写到引用时还没验完） | 高 | refs 路按桶 A→B→C→D→E 排序，A/B 桶（绪论用得最多）优先验证 |
| docx 导出后学校格式偏差 | 高 | 接受——专门写 `format-checklist.md` 让用户手调，不强求 pandoc 完美 |
| 模型自动生成的中文有「AI 味」 | 高 | 强制每章过去-AI-味 prompt；最终 Reviewer 自审 |

---

## 11. 不做什么（YAGNI）

- 不写 LaTeX 版本（用户明确说交付 docx 即可）
- 不接外部 baseline（AutoGen / LangGraph 跨语言比较不公平，方案 A 已排除）
- 不强一致复制的设计（fuxi 当前阶段就不是这个范围，第 4 章如实说明）
- 不补 demo 视频或 PPT（如果答辩需要再单独做，不在本 spec 范围）
- 不做英文版正文（学校只要中文版 + 英文摘要）

---

## 12. 验收标准

spec 阶段（本文档）：
- [ ] 用户 review 后认可所有决策（章节、字数、refs、图表、实验）
- [ ] commit 进 git

写作完成后：
- [ ] 正文 ≥ 24000 字（≤ 28000 字最好）
- [ ] 30 篇参考文献全部在正文有引用（无悬挂引用）
- [ ] 每篇参考文献都有 search 证据（refs-verification.md）
- [ ] 6 个公式编号闭合，正文有引用
- [ ] 图表全部有正文引用，编号闭合
- [ ] 通过 awesome-ai-research-writing Reviewer 自审一轮
- [ ] pandoc 导出 docx 成功
- [ ] format-checklist.md 给到用户

---

附：本文档由 brainstorming 阶段决策集合而成，不可被下游 agent 单方面改动；如需修改，先回到本会话提出修订。
