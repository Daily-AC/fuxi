# thesis-v3 Session A handoff

**Session 范围**：2026-05-07 下午到傍晚，本会话从 v2 完成 → 用户批评「糊弄 + 浅 + 图烂 + a2a 没写 + 格式不对」→ 决定重做 v3 LaTeX → 三 brief 调研 + 三章 v3 写完。

**接班人**：你（在新会话里）。请先读 CLAUDE.md，再读本文件，再读下面列的 3 份 brief，再开干。

---

## 1. 一句话现状

v3 LaTeX 论文：基础 + 三 brief 全到位 + ch 3/5 已写完 commit + ch 4 agent 仍在跑（接班时极大概率已完工，去拉就好）+ ch 1/2/6 + figures 重做 + LaTeX 编译验证 仍待做。

**关键阻塞**：xelatex 工具链装到一半，要用户跑一行 sudo 才能编译验证 LaTeX。命令在 §6。

---

## 2. 用户档案与硬性偏好（必读）

- 名字：以琳。学校：湖南第一师范学院。本科毕设。题目：**基于 AI Agent 的高性能分布式通讯系统**（用「通讯」不是「通信」，封面/扉页/页眉四处必须一致）
- 个性：希望被反驳不要讨好；批评直接但接受 push back；情绪化道歉过一次（不要为此谨慎，继续直接说）
- 工作模式：
  - 不设 DDL，干完为止（CLAUDE.md 公理 7：毕设是顺带）
  - 全权限不弹 yes/no
  - 后台 agent 工作时禁 git add -A
  - 决策三档：公理/可见行为/内部实现 默认行为，反公理才打招呼
- v2 教训（user 的原话提炼，必须避开）：
  - 「感觉挺差的，跟顶刊查的太远了，感觉像随便糊弄的」
  - 「论文写的太表层了，感觉不太深入」
  - 「format 基本不对」「docx 你根本不擅长」→ 改 LaTeX
  - 「matplotlib 和 gpt 的图没有任何顶刊水平」
  - 「a2a rust 没有现成 sdk 是我们从零实现的，这块要不要写？」→ **必须写**，A2A 从零实现是论文核心贡献
  - 「gpt-image-2 别用来生成 q 萌图了」→ 用学术 illustration 风格
- v3 决策：
  - 路线：B 偏厚重升级到 30K-50K 字
  - 输出：LaTeX 写 + xelatex 编 PDF + pandoc latex→docx 转
  - figures：matplotlib 数据图 + draw.io/TikZ 结构图 + gpt-image-2 hero illustration（不萌）
  - 30 篇 refs 直接复用 v2 已验证的（refs.bib + verification.md 都拷过来了）

---

## 3. v3 文件布局

```
deliverables/thesis-v3/
├── main.tex                                # ctexart + 学校格式 ✅
├── build.sh                                # xelatex+biber+xelatex+xelatex 编译脚本 ✅
├── frontmatter/
│   ├── cover.tex                           # 封面 ✅
│   ├── title-page.tex                      # 扉页 ✅
│   ├── honesty.tex                         # 诚信声明 ✅
│   ├── abstract-cn.tex                     # 中文摘要（含 A2A 贡献） ✅
│   └── abstract-en.tex                     # ABSTRACT（含 A2A 贡献） ✅
├── chapters/
│   ├── 01-introduction.tex                 # ❌ 待写（v3 6K 目标）
│   ├── 02-overall-design.tex               # ❌ 待写（v3 6K 目标）
│   ├── 03-modules.tex                      # ✅ 9051 字 / 5 段 Rust 代码 / 15 refs
│   ├── 04-implementation.tex               # 🔄 agent a83005 在跑（极大概率已完）
│   ├── 05-experiments.tex                  # ✅ 10081 字 / 8 refs / 6 表
│   ├── 06-conclusion.tex                   # ❌ 待写（v3 2-3K 保持）
│   └── 99-acknowledgements.tex             # ❌ 待写（v3 ~500 字）
├── bib/refs.bib                            # ✅ 30 篇 v2 验证过的
├── refs-verification.md                    # ✅ 30 篇 search 证据链
├── research/                               # ⭐ v3 三大 brief，必读
│   ├── a2a-from-scratch.md                # 26KB / 809 行 → ch 3.2 + 4.5 素材
│   ├── figure-design-conventions.md       # ~14KB / 350 行 → 14 张图 P0 改进点
│   ├── fuxi-architecture-brief.md         # 65KB / 2066 行 → ch 3-4 各模块素材
│   └── gpt-image-academic-strategy.md     # gpt-image-2 学术风用法 + fig-1-1 重做 prompt
├── figures/
│   ├── style.py                            # ✅ Okabe-Ito + Tufte + PDF 顶刊版
│   ├── matplotlib/                         # ❌ 6 张实验图全部待重做（v2 PNG 不能直接用）
│   ├── drawio/                             # ❌ 8 张架构图待重做（按 Aegaeon 风格）
│   └── gpt-image/                          # ❌ fig 1.1 待重做（去萌系，按新 prompt）
├── benchmarks/                             # 实验数据
│   ├── (latency-samples.csv)               # 等 cargo bench 重跑后产出，给 fig 5-4/5-5 eCDF 用
│   └── ❗ v3 未独立存数据，v2 的 v2-2026-05-07.md 是源
├── templates/
│   ├── 学校模板.doc
│   └── 格式规范注意事项.docx
└── handoff/
    └── v1-session-A.md                     # 本文件
```

v2 也保留在 `deliverables/thesis-v2/`，是历史对照（不要再用，只看不改）。

---

## 4. 三 brief 摘要（接班必读）

### 4.1 a2a-from-scratch.md
A2A 协议代码级详解，**论文核心差异化贡献**。要点：
- crate `fuxi-a2a` 总 1321 LOC，wire 458 LOC + server 196 + client 203 + tests 258
- A2A v1.0 五个 RPC 方法（agent/getCard, tasks/send, tasks/sendSubscribe, tasks/get, tasks/cancel）
- **InputRequired 状态创新**（Google A2A spec 没有，fuxi 自加）
- **单连接 HTTP+SSE 设计**（同 endpoint，sendSubscribe 时升级 SSE）
- 5 段可贴论文的代码片段（每段 5-15 行）

### 4.2 figure-design-conventions.md
顶刊 figure 风格调研，**最佳参照 paper：SOSP'25 Aegaeon (Alibaba)**。要点：
- 14 张图（6 mat + 8 drawio）每张的 P0 改进点
- 反模式：dual y-axis / 图内 title / log+stacked / 红绿同框 / PNG 输出
- 已抄到 `v3/figures/style.py` 的 Okabe-Ito 8 色 + PDF fonttype=42 + Tufte 风
- 重大改图建议：
  - fig 5-4 / 5-5 bar→eCDF（需要 raw samples，见 §6 阻塞）
  - fig 5-6 log+stacked → 拆 1×3 linear-x subplots
  - 8 张架构图统一调色：白底 + 浅灰 + 浅蓝 三色 + dashed 跨进程边界 + 编号气泡 ① ② ③

### 4.3 fuxi-architecture-brief.md
全 13 crate 代码级 brief，**论文 §3-§4 各模块素材源**。要点：
- 总 LOC ~72,720
- 5 个工程亮点（按论文价值排序）：
  1. 非阻塞 EventBus + lag 哨兵
  2. broadcast + mpsc 职责分离
  3. 三层沙箱 L1/L2/L3
  4. WS 反连 agent 模式
  5. Event enum + exhaustive match
- 每 crate 都给了 path:line 精确定位 + 可贴 Rust 代码片段

### 4.4 gpt-image-academic-strategy.md
gpt-image-2 用法策略（user 反馈 v2 萌图走偏后重写）。要点：
- 三工具分工：matplotlib（数据）/ draw.io（结构）/ gpt-image-2（hero illustration）
- gpt-image-2 只在 ch 1 motivation hero + ch 6 future work hero 用
- 给了 v3 fig-1-1 重做 prompt：editorial flat vector / isometric / 抽象几何 / 3-4 色 / 无文字 / 16:9
- 用户对此 prompt **未明确表态**——首次跑前可以问一下确认

---

## 5. v3 章节深挖标准（已贯彻在 ch 3/5，ch 1/2/4/6 沿用）

每章必有：
- 章首引言 ~200-300 字
- 真实代码片段（lstlisting Rust，不是伪代码）
- 关键设计决策段（为什么这样选，对比 alternatives）
- 真实 cite key（refs.bib 的 30 个）
- 章末小结 ~300 字 自然衔接下一章

字数预算（v3 比 v2 大幅 +）：
| 章 | v2 | v3 目标 | 状态 |
|---|---|---|---|
| 摘要 | 580 | 700 | ✅（A2A 贡献已凸显） |
| 1 绪论 | 5050 | 6000 | ❌ 待写（贡献明确列表必加 4-5 条，A2A #1） |
| 2 总体设计 | 4300 | 6000 | ❌ 待写（加需求-设计映射表 / SLA 公式） |
| 3 模块设计 | 6700 | 9000 | ✅ 9051 字 |
| 4 系统实现 | 6800 | 10000 | 🔄 agent 在跑（A2A §4.5 ≥3500 字必到位） |
| 5 实验 | 7560 | 10000 | ✅ 10081 字 |
| 6 总结 | 2070 | 2-3K | ❌ 待写 |
| 致谢 | 500 | 500 | ❌ 待写 |
| **合计** | 33500 | **45-50K** | **~19K 已写** |

---

## 6. 阻塞与待办（接班的活）

### 6.1 立即查 ch 4 agent 状态

```bash
# agent ID
cat /Users/e0_7/.claude/projects/-Users-e0-7-fuxi/639290b3-6a94-4d94-bb5e-ae6b559bb506/subagents/agent-a83005996d77fd4ea.jsonl | tail -1
# 或者直接看产物
ls deliverables/thesis-v3/chapters/04-implementation.tex
```

如果 ch 4 已写好：commit it（参考 ch3/5 commit message 模式）。

### 6.2 用户必须输 sudo 装 xelatex

basictex .pkg 已下载到 `/opt/homebrew/Caskroom/basictex/2026.0301/`，brew install 因 sudo 非交互失败。让用户跑：

```bash
sudo installer -pkg /opt/homebrew/Caskroom/basictex/2026.0301/mactex-basictex-20260301.pkg -target /
```

装完确认：
```bash
eval "$(/usr/libexec/path_helper)" && which xelatex && xelatex --version | head -1
```

可能还要 tlmgr install 几个包：
```bash
sudo tlmgr update --self
sudo tlmgr install ctex xecjk fontspec biblatex biber gb7714-2015 \
                   booktabs longtable multirow tabularx fancyhdr \
                   titlesec setspace geometry caption listings hyperref
```

### 6.3 待写章节（ch 1/2/4 review/6/致谢）

- **ch 1 绪论 v3** (~6K 字)：v2 5050 字基础上加「研究目标与贡献」单节明确 4-5 条贡献，**A2A 从零实现是关键贡献 #1**。建议 dispatch agent，仿 ch 3/5 prompt 模板（在本会话上下文中）。
- **ch 2 总体设计 v3** (~6K 字)：v2 4300 字基础上加需求-设计映射表 / 形式化模块边界 / SLA 公式。
- **ch 4 系统实现 v3** (~10K 字)：等 agent a83005 完工 → 验收（A2A §4.5 是否到 3500+ 字 / 是否真 Rust 不是伪代码）。
- **ch 6 总结 v3** (~2-3K)：v2 2070 字基础上微扩。
- **致谢** (~500 字)：v2 已有，照抄改为 .tex。

### 6.4 figures 全部重做（按 figure-design-conventions §3）

**已加 latency.rs sample dump 代码**（commit 在 cargo check 还没回，下一步要测）：

```bash
# 验证修改编过
cargo check -p fuxi-cli --benches
# 重跑 baseline 让 latency-samples.csv 生成（预计 ~8 分钟）
cargo bench -p fuxi-cli --bench run_baseline
ls deliverables/thesis-v3/benchmarks/latency-samples.csv
```

然后写 6 张图的 v3 plot 脚本（直接用新 style.py），输出 PDF：
- fig-5-1 scalability：1×2 subplots（throughput + efficiency），删 dual y-axis
- fig-5-2 poll-scan：折线 + 95% CI shaded
- fig-5-3 bus-stress：自家深蓝实线 + baseline 浅灰
- **fig-5-4 dispatch-latency: bar 废，改 eCDF**（需 latency-samples.csv）
- **fig-5-5 event-flow-latency: 同 5-4 改 eCDF**
- fig-5-6 e2e-breakdown：拆 1×3 linear-x subplots

8 张架构图改用 TikZ 重做（style 见 figure-design-conventions §2.1，3 色 + dashed 跨进程边界 + 编号气泡）。

fig 1-1 hero：
- 检查 user 是否同意 gpt-image-academic-strategy.md 里的新 prompt
- 同意 → 调 gpt-image-2 skill 生成

### 6.5 LaTeX 编译验证

xelatex 装完后：
```bash
cd deliverables/thesis-v3 && ./build.sh
```

预期会有 LaTeX 报错（中文字体可能要调 / lstlisting Rust 高亮可能需 utf8 / cite key 拼写错等）。逐个修。

成功后产出 `main.pdf`。

### 6.6 latex2docx（可选，看用户要不要）

学校真要 docx 时：
```bash
pandoc main.pdf -o thesis-v3.docx  # 不行，PDF→docx 质量太差
# 改用：
pandoc -s main.tex -o thesis-v3.docx --bibliography=bib/refs.bib --citeproc
# 或者把章节 .tex 拼成单文件再转
```

---

## 7. 本会话已完成事项 commit log

```
fafb747 draft(thesis): v3 第 3 + 5 章 LaTeX 初稿（agent 并行写）
cddcf7f research(thesis): fuxi 全 13 crate 代码 brief · 65KB / 2066 行 · 5 工程亮点
9dc06b4 chore(thesis): v3 style.py 顶刊版 + gpt-image-2 学术策略 doc
ce3036d research(thesis): 顶刊 figure 风格调研 brief
032b547 chore(thesis): v3 LaTeX 基础 · main.tex + frontmatter + 复用 v2 refs.bib + a2a brief
72948a5 chore(thesis): 学校模板与格式规范归档进 fuxi/deliverables/thesis-v3/templates/
```

之前的 v2 完整工作 commit 链不再列（看 `git log --oneline 04eb34c..main`）。

---

## 8. 已识别的潜在风险

1. **ch 4 agent 可能输出仍偏伪代码**：a83005 prompt 里我反复强调「真 Rust 代码 / 不要伪代码」，但 brief 里给了样例片段，agent 大概率会贴。验收时重点查 lstlisting 数（应 ≥ 5 段）+ 字数（应 ≥ 10K）。
2. **xelatex 装机依赖用户 sudo**：除非用户输密码，否则 build.sh 跑不起来。可以等 ch 1/2/6 写完一并验证。
3. **fig 5-4/5-5 eCDF 数据**：需 latency.rs mod + cargo bench 重跑（8 min）。如果 user 不想等，可以保留 bar 但用新 style.py 美化。
4. **gpt-image-2 fig 1-1 prompt user 未确认**：建议接班时先问 user 是否 OK 新 prompt，再生成。
5. **学校格式真到 docx 才稳**：pandoc tex→docx 可能丢 lstlisting 高亮 + 公式编号格式。最后必须打印 PDF 校对一遍。

---

## 9. 接班开场建议

新会话 `/clear` 后，把如下作为第一条 prompt：

```
读 /Users/e0_7/fuxi/CLAUDE.md 和 /Users/e0_7/fuxi/deliverables/thesis-v3/handoff/v1-session-A.md，
然后告诉我 v3 论文当前完成度 + 下一步建议做什么。
```

接班后我会按 handoff doc §6 优先级推进。如果 ch 4 agent 已完工，第一件事就是 commit 它。
