# 何时呼叫玄女审阅（deliverable nudge）

我中间过程的所有事件玄女**默认不读**——`AgentResponded` / `ToolCallStarted` /
`ToolCallFinished` 都只留痕在 EventBus，玄女注意力是稀缺资源，留给真正的交付。

呼她唯一的方式是发 `AgentRequestReview` 事件——平台会震一下她。所以**判断
"什么时候该震"** 是工匠的本分：少了用户失去过程感；多了玄女被噪声淹没（退化成
"每改一个文件就 nudge" = attention 模型崩溃）。

## 5 类 deliverable

发出时必须带一个 `deliverable_kind`。下面是**何时该发** + **何时不该发**。

### 1. `research_summary` — 我搞清楚了某件事

**发**：完成一个**独立的调研主题**，结论可让玄女据此决策下一步。
- 读懂某模块的契约、定位 bug 的根因、对比两种方案选定一种
- "ClickHouse vs Datafusion 我看完了，推荐 X，理由 1/2/3"

**不发**：还在 grep / 才读了一两个文件 / 心里有猜想没验证。

### 2. `code_change` — 我交了一段能跑的代码

**发**：写完一段**能编译、本地三绿（fmt + clippy + test）**的代码，等授权 commit。
- 一个 feature 的最小绿、一个 bug fix 的最小绿
- 多个 deliverable 的大改也分批：能独立 review 的一段 = 一次 nudge

**不发**：写到一半还红 / 才改一个文件 / 顺手 ref. 中 / 测试还没跑。

### 3. `test_result` — 我有了一组有结论的测试结果

**发**：跑完一轮测试、有可交付的"通过 / 失败 / 数字"。
- 覆盖率达标、性能 benchmark 出数、引入的回归被复现
- 复杂的多 case 矩阵跑完一组

**不发**：测试还在跑 / 跑到一半挂了重跑中 / 单测随手过没人问。

### 4. `decision_request` — 我遇到只有玄女能定的取舍

**发**：自己已分析过 trade-off，把 ≥2 个选项 + 我的推荐摆给她。
- 两种实现方向、API 命名争议、要不要破坏兼容、要不要扩 deliverable_kind 枚举

**不发**：不是真二选一（明显该选 A 还在问） / 没自己想过就甩问题给她。

### 5. `error_block` — 我卡死且自救失败

**发**：试过 ≥2 个假设都不对、连续 ≥3 步走不通同一个根因；或缺权限 / 缺资源 /
环境异常自己处理不了。继续试就是浪费 token。

**不发**：第一次 cargo build 红就喊救命 / 试一次没过就 nudge / 是该等授权而不是
该报错（授权类停在 `awaiting_*` 状态等玄女传话，不发 `error_block`）。

## 通用反例（无论哪一类都不要做）

- **中间过程播报**——"我现在在做 X" 不是 deliverable，是噪声
- **每改一个文件就 nudge**——退化成 attention 灾难，等于回 A 模式
- **整个 task 做完才发一次**——多 deliverable 之间分批 nudge，让玄女有过程感
- **自己能解决的事**（授权类除外）——能 fix 就 fix，nudge 不是甩锅入口

## 字段填法

- `summary` — 一两句中文，可让玄女**不用 recall** 就能决策（"鲁班升级了 rust
  1.75，三绿，等授权 commit"）。不要复述自己干了啥的过程。
- `artifact_ref` — 可空：纯摘要类（如 `research_summary` / `decision_request`）
  可只附 `summary`；`code_change` 应填 commit sha 或 diff path；`test_result`
  填 log path 或 benchmark 数据 path。
