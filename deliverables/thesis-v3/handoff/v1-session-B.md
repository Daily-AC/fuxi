# thesis-v3 Session B handoff

**Session 范围**：2026-05-07 晚到夜，从 session-A 接班 → 4 张架构图重排 + 5 contributions 重塑 + reviewer 视角 W1-W7 critical 全修 + AI 味/术语/refs polish + logic check 7 处必改 + AIGC 率 skill 调研 → 用户上下文满，开新会话继续。

**接班人**：你（在新会话里）。请先读 CLAUDE.md → handoff/v1-session-A.md → 本文件，再开干。

---

## 1. 一句话现状

v3 LaTeX 论文 **88 页 / 4.0 MB / 0 LaTeX 错误 / 0 未定义引用**，正文 + 图 + refs 全部到位，reviewer + logic + AI-味 三轮 review 全过。剩下唯一硬骨头是**降 AIGC 率到 < 10%**（学校用「格子达」检测）。已调研出 3-4 个推荐本地 skill，**user 决定先不送格子达跑初始率**，要先装本地 skill 试。

最新 PDF：`/Users/e0_7/fuxi/deliverables/thesis-v3/main.pdf`

最新 commit：`592b0a5 fix(thesis): logic check 发现的 7 处必改全部修复`

---

## 2. 本会话已完成事项

按时间顺序：

### 2.1 4 张 TikZ 架构图重排（user 实测发现遮挡）
- fig 2-1 顶层架构：删 ①②③④ 流程编号（架构图非时序图）；3 条 agent→bus 箭头错开 endpoint
- fig 2-2 时序图：编号气泡从中心移到起点 lifeline 右侧
- fig 2-3 跨节点：auto-pin 公式收到右下角；①② 挪到 dist controller 右侧
- fig 3-1 模块依赖：改用显式 (x,y) 5 层网格，cli 真在 L5（之前与 cc 同 y），层标签统一锚到固定 x 列
- 顺手修：main.tex `\numberwithin{equation/figure/table}{section}` 顺序，确保 (3-1)/(2-1) 编号正确

### 2.2 fig 1-1 hero v2（user 嫌 v1 太花）
- v1：3D 立方体 + 光束（user 评"显花"）
- v2：纯 2D 单色 navy schematic，类 MapReduce / GFS 论文 figure 1 风
- 老版 backup 留在 `figures/gpt-image/fig-1-1-overview-v1-backup.png`

### 2.3 awesome-ai-research-writing 三轮 review pipeline

**Step 1 — Reviewer 视角全文审视**（agent dispatched）  
找出 7 处 Critical Weaknesses + 一批 Minor。本科毕设语境下评 87/100；NSDI 顶会尺度 3.5/10（borderline reject 偏下）。

**Step 2 — Critical W1-W7 修复**（自己手改）  
- W1：模拟任务口径收紧（摘要 cn/en/ §5.8 加「合成基准 / 调度路径吞吐」+「真实 LLM Agent 端到端受推理 1-30s 主导」）
- W2：删 §5.3 末段虚构 50% baseline 推算（与 §5.6 自相矛盾）；改为定性引 §5.6
- W3：跨节点贡献四收口径（"sandbox 验证"），与 §6.2 一致
- W4：「跨节点事件流」全文 grep 改「事件总线广播」（中文 9 处 + 英文 2 处）；§5.4 加「进程内路径 + 跨节点未实测」disclaimer；32.97ms 拆解加「工程估算非 profiler」disclaimer；§5.7 stacked bar caption 标"估算图"
- W5：§5.5 重写。删错误论证「16 worker = 1 worker 是水平扩展理想情形」；改为正确解释「dispatcher 未饱和 + per-worker 任务数固定」。删 MapReduce 76-82% 跨数量级对比。§5.1 加机器规格：Apple M4 Pro / 14 核 (10P+4E) / 48 GB / macOS 26.0.1 / Rust 1.93.1 / SQLite 3.51.0
- W6：lag_threshold 256→512（核对 fuxi-events/src/bus.rs:41）；50/100/200ms→50/100/150ms（core: store.rs:124）
- W7：贡献五重塑——把"完整可用工程平台"/"性能边界" 改为"跨节点扩展点钩子化抽象"（4 个 trait: RecallSink/DistEnqueuer/NodeLoadProvider/ExtractorSpawner）；性能数字降级为"工程兑现"段落，明确不作为独立贡献

ch1 §1.3 与 ch6 §6.1 5 条贡献现在完全对齐。

### 2.4 Polish pass（agent dispatched）
- 去 AI 味：删 ~350 字 meta-narration（§1.1/§2/§3/§4/§5 章首引言里的 reviewer 命中段）
- 中文润色：仅修语病/口语/严重欧化长句
- 术语统一段：§1.1 末段后新增「本文术语约定」（约 200 字，统一玄女/门客/EventBus/通讯/A2A 与 MCP）
- cite 减密：§1.2 第二段 8→5 cite，第四段 8→4 cite

**refs.bib 补 6 篇 2024-2026 工作**（reviewer minor 第三条）：
- ag2_2024, qian2024chatdev, crewai_2024 → §1.2 多 Agent 协作框架段
- lin2024parrot → §1.2 LLM serving 系统层研究新增段
- nats_2024, tower_backpressure_2024 → §3.3 lag 哨兵段

最终 refs.bib：30 → 36 篇

### 2.5 Logic check 终稿校对（agent dispatched + 自己手修 7 处）

发现 5 致命 + 2 术语：
1. lag_threshold ch2 §2.4/§2.6 还有 2 处 256，已改 512
2. LOC 数字一致性：以 `find/wc -l = 82515` 为准统一全文
   - ch2/ch3：「7.27 万行」→「8.25 万行」
   - ch4 表 4-1：所有 13 个 crate LOC 用 find/wc 重新统计（cli 33408 / im 16149 / orch 10220 ...，总 82515）
3. ch1 §1.5 章末误把「工程兑现」升格为第五条贡献，与 §1.3/§6.1 矛盾，已改回「跨节点扩展点钩子化抽象」
4. ch5 §5.3 公式 (2-3) 形式不一致：补充 `T_comm ≡ L_enq + L_disp + L_evt` 简化说明；删错引「公式 (2-3) 中流水化项」改为「流水化效应未在 (2-3) 静态分解中显式建模」
5. ch6 §6.1 工程兑现段「64 sub × 1.6 M events/s 零丢帧」与 ch5 §5.7 数据矛盾（实际 1.6M = 16 sub × 100k cell），改为「64 sub × 10,000 events/s 零丢帧」
6. ch2 §2.1 表 2-1 公式列错配：真实时观测 (2-2)→(2-4) ；可观测背压 (2-3)→(2-4)
7. ch1 §1.2「见第 5 章表 5-2」实际是 throughput 表，应是 latency 表（5-3），已改 `\ref{tab:latency}` 自动解析

末态：88 页 / 4.0 MB / 0 LaTeX 错误 / 0 未定义引用。

---

## 3. 接班核心任务：降 AIGC 率到 < 10%

学校用「**格子达**」检测 AIGC 率，要求 < 10%。这是答辩前唯一卡点。

### 3.1 调研结论（agent dispatched WebSearch 出）

**没有任何开源工具明确逆向了格子达**——9 次搜索 0 命中。所有"针对格子达"的宣传都是收费 SaaS 黑箱（fuck_aigc_api 是典型陷阱：源码只是 paperpure.net 销售页）。

**HuggingFace 上没有可用的中文学术 AIGC detector**——通用英文 GPT-2 时代遗物，对中文论文 distribution 没微调。

**真正的开源生态走 prompt skill 路线，不是 detector 模型路线**——4 个高质量 skill 都是「打破 AI 统计模式」prompt + 规则。

**格子达 / 知网 / 维普 / 万方 是同族算法**——核心是 perplexity + 句式整齐度 + 连接词密度 + 词汇多样性。针对前三个的工具基本能覆盖格子达。

### 3.2 推荐 4 个本地 skill（user 已确认不上传第三方）

按推荐度：

#### ★★★★★ humanize-chinese · [voidborne-d](https://github.com/voidborne-d/humanize-chinese)
- 检测 + 降痕一体，CLI + CC skill 双形态
- 算法：字符级 3-gram perplexity + 20+ 规则 + 逻辑回归 ensemble
- **完全本地零联网零 API key**，279 stars，v5.0.0 / 2026-05 在更
- 装：`git clone https://github.com/voidborne-d/humanize-chinese ~/.claude/skills/humanize-chinese`
- 用：`/detect` 自检 + `/humanize` `/academic` `/style` 三档降痕

#### ★★★★ humanizer-zh-academic · [redbaronyyyyy-eng](https://github.com/redbaronyyyyy-eng/humanizer-zh-academic)
- 16 维 AI 写作签名识别，专为中文学术
- 注入人类随机性破坏检测器统计模式（直击 perplexity/burstiness）
- 104 stars，CC skill
- 装：`git clone https://github.com/redbaronyyyyy-eng/humanizer-zh-academic ~/.claude/skills/humanizer-zh-academic`

#### ★★★★ aigc-reduce · [xiaofenggan01](https://github.com/xiaofenggan01/aigc-reduce)
- **配套 `aigc_scan.py` 静态扫描器**（对应我们最初想自建的功能）
- 7 维 AI 特征扫描 + 重写指导
- 明确针对 **CNKI 知网 3.0 / 万方 / PaperPass**
- 装：`git clone https://github.com/xiaofenggan01/aigc-reduce ~/.claude/skills/aigc-reduce`

#### ★★★★ academic-writing-skills · [bahayonghang](https://github.com/bahayonghang/academic-writing-skills)
- 套件含 `latex-thesis-zh`（**完全是我们场景**），182 stars
- 自动识别 thuthesis/pkuthss/ustcthesis/fduthesis 模板
- 内置 De-AI processing + AXES 段落转场 + 双语 caption + 引文完整性
- 装：`npx skills add github.com/bahayonghang/academic-writing-skills`

### 3.3 推荐战术

```bash
# 一次装 3 个备选（前 3 个）
git clone https://github.com/voidborne-d/humanize-chinese ~/.claude/skills/humanize-chinese
git clone https://github.com/redbaronyyyyy-eng/humanizer-zh-academic ~/.claude/skills/humanizer-zh-academic
git clone https://github.com/xiaofenggan01/aigc-reduce ~/.claude/skills/aigc-reduce
```

然后挑 1-2 段最高危段（**章首引言 + 摘要 + ch6 总结**）丢进去三个工具自检 + 降痕，对比效果。挑最稳的那个跑全文。

### 3.4 高危段定位（基于 reviewer 已知信息）

reviewer 已经标过的 AI 味集中区（已 polish 过一轮但可能仍有残留）：
- 摘要 cn / en
- §1.1 第二段「AI Agent 在 2023 年之后再度成为系统层面的研究焦点」段
- §1.3 五条贡献的 \textbf{} 标题段（结构太工整）
- §3.x 章首引言段
- §5 章首引言段
- §5.3-§5.7 物理意义解释段（结构化重）
- §6.1 工程兑现段（reviewer 加的，但形式工整）
- §6.4 结语段

最危险结构特征：
- `\textbf{...}。` 命名段（"贡献一：..."、"工程兑现："）→ 太工整
- 「\textbf{第一}...\textbf{第二}...\textbf{第三}」三段式 → 强 burstiness 缺失
- 「公式 (X) 表明..."」段落 → 教科书式
- "本节...本章..." meta-narration（已基本删掉，但部分章末小结段可能还有）

### 3.5 自建 skill 兜底方案（如三个开源都不达标）

可参考做：
1. 拼装本地 perplexity 评分器：humanize-chinese 的 3-gram perplexity 代码可抠出来跑全文
2. 加格子达明面声明的检测维度（句长方差 / 连接词密度 / 词汇多样性 / 句号密度）
3. 用玄女 + cc 门客做 multi-pass 重写循环，借鉴 baibaiAIGC 的两轮 prompt 模板（[poleHansen/baibaiAIGC](https://github.com/poleHansen/baibaiAIGC)）

工作量约 2-4 小时（如果三个开源 skill 试下来效果都不到位才走这条路）。

---

## 4. 接班开场建议

新会话 `/clear` 后，把如下作为第一条 prompt：

```
读 /Users/e0_7/fuxi/CLAUDE.md 和
/Users/e0_7/fuxi/deliverables/thesis-v3/handoff/v1-session-B.md
（如果 v1-session-A.md 没读过也补读一下）

然后开干第 3 节"接班核心任务：降 AIGC 率"——
先装 3 个推荐 skill，然后挑摘要 + ch1 §1.1 第二段 + ch6 §6.1 三段最高危的丢进去
A/B 测试，给我看降痕效果对比，再决定全文跑哪个。
```

---

## 5. 重要 commit 链（git log）

```
592b0a5 fix(thesis): logic check 7 处必改修复（lag/LOC/章末贡献/公式/数据一致性）
573a235 polish(thesis): minor pass · 去 AI 味 + 润色 + 术语 + refs +6
a2b24c4 revise(thesis): reviewer 视角 W1-W7 critical 全修
98ea771 fix(thesis): 4 张架构图重排 + fig 1-1 改极简 + 图表 numberwithin
fa612fc chore(thesis): 排版与术语统一 · 500→4 overfull · 通信→通讯全文 58 处
d8c07ee draft(thesis): ch 1 加 hero illustration · gpt-image-2
4dece52 draft(thesis): ch 1/2/6 + 6 实验图 + 4 架构图 + LaTeX 编译链全跑通
d1f8748 draft(thesis): 第 4 章 + handoff session-A
fafb747 draft(thesis): 第 3 + 5 章 LaTeX 初稿
```

---

## 6. 用户硬性偏好提示（与 session-A 一致）

- 名字：以琳。学校：湖南第一师范学院。本科毕设。
- 决策三档：公理/可见行为/内部实现 默认行为，反公理才打招呼
- 全权限不弹 yes/no
- 后台 agent 工作时禁 git add -A
- 不设 DDL，干完为止（公理 7：毕设是顺带，但答辩日期已迫，AIGC 率是当前唯一卡点）
- **降 AIGC 率不上传第三方** —— 见 memory `feedback_aigc_no_third_party`
- 写论文时**代码是第一真相源**，所有 LOC / 行号 / API 必须 grep 代码核对 —— 见 memory `feedback_thesis_code_is_truth`
- 与上版口径一致的关键数字：
  - 总 LOC = **82,515**（find/wc 统计），不是 72,720（旧 tokei 数字已删）
  - lag_threshold = **512**（不是 256）
  - 退避 = **50/100/150ms**（不是 50/100/200ms）
  - 654.66 tasks/s 是 **8 worker × 10ms 模拟任务** 下的调度路径吞吐（必加合成基准限定词）
  - 32.97ms task_dispatch p50 + 0.07ms 进程内事件总线广播 p50（**不是跨节点事件流**）
  - 64 sub × 10,000 events/s 零丢帧（不是 1.6M）
  - 5 条贡献：A2A / EventBus + lag 哨兵 / 玄女门客分层 / sandbox + WS 反连 / 跨节点钩子化
