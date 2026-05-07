# 顶刊系统论文 Figure 设计模式调研

> 调研对象：SOSP 2024、OSDI '24、NSDI '24、SIGCOMM '24、EuroSys '24/'25、USENIX ATC '24 中**系统类**论文。
>
> 目标：把 v3 论文里 6 张 matplotlib 实验图 + 8 张 draw.io 架构图，从「能看」拉到「顶会作者一眼信服」。
>
> 调研日期：2026-05-07

---

## 0. 现状快照（v2 → v3 改进的起点）

`thesis-v2/figures/matplotlib/style.py` 当前配置：
- `figsize=(6.5, 4.0)`、`dpi=300`、`font.size=11`、CJK 走 PingFang SC
- 调色板硬编 tab10 子集（蓝 / 橙 / 绿 / **红** / 紫 / 棕）
- `axes.grid=True / grid.alpha=0.3`、`spines.top/right=False`
- 输出 PNG，不是 PDF/SVG

主要 gap（看了 5.1 / 5.4 / 5.6 三张实样图后总结）：

1. **调色板里有红绿同框**（fig-5-4），过 colorblind 模拟会糊成一团
2. **图内带 title**（"任务派发延迟分布 (n=500, 5-run)"），顶刊一律不写——title 是 caption 的活
3. **dual y-axis** 在 fig-5-1 用了，systems 顶会基本不用（容易误导）
4. **breakdown 图 log x-axis** + 堆叠柱：log + stacked 是禁忌组合，宽度不再是真值
5. PNG 输出，submitted 给评委会被压缩失真——必须 PDF/SVG

下面是顶刊论文里反复出现的 12 条公约。

---

## 1. 总体观察

| 维度 | 顶刊主流做法 | 数值/取值 |
|------|--------------|-----------|
| **正文字体** | sans-serif（少数 SOSP 老牌组用 Times/CMR） | 论文正文 9-10 pt → 图内 7-8 pt |
| **轴刻度字号** | 比正文小 1-2 pt | 7-8 pt |
| **图例字号** | 与轴刻度同字号或小 1 pt | 6-7 pt |
| **图宽（双栏 venue 单列图）** | 3.3-3.5 inch（85 mm，PRL/ACM 两栏惯例） | width = 3.487 in，height = width / 1.618 ≈ 2.15 in |
| **图宽（横跨双栏的总图）** | 6.5-7.0 inch | width = 7 in |
| **DPI** | 300（如果非 vector） | savefig dpi=300 |
| **首选格式** | **PDF** > SVG > EPS；只在必要时 PNG | LaTeX `\includegraphics{*.pdf}` 直插 |
| **颜色数** | 同图最多 4-5 种主色，3 种是甜区 | 超过 5 种 → 拆图或加重灰阶差 |
| **网格** | 多数**不画**；如画，只画水平线 + alpha ≤ 0.3 + 灰色 | `grid(axis='y', color='0.85', linewidth=0.5)` |
| **spine** | 隐藏 top + right；left + bottom 保留细线 | `spines.top/right=False`，`axes.linewidth=0.5` |
| **tick 朝向** | 多数**朝内**（in） | `xtick.direction=in / ytick.direction=in` |
| **图内 title** | **不写**！title 一律走 LaTeX caption | `ax.set_title(...)` 必删 |

宽度具体怎么填：
```python
ONE_MM = 1 / 25.4
SINGLE_COL_WIDTH = 85 * ONE_MM     # 3.346 in，ACM/USENIX 单列
DOUBLE_COL_WIDTH = 178 * ONE_MM    # 7.008 in，跨双栏
GOLDEN = 1.618
single_size = (SINGLE_COL_WIDTH, SINGLE_COL_WIDTH / GOLDEN)
double_size = (DOUBLE_COL_WIDTH, DOUBLE_COL_WIDTH / 2 / GOLDEN)
```

---

## 2. 各类图的具体模式

### 2.1 系统架构图（layered / module / data-flow）

**论文样例 A：CASSINI（NSDI '24, MIT）Fig 2 系统架构**
- URL: <https://www.usenix.org/system/files/nsdi24-rajasekaran.pdf>
- 关键模式：
  1. 仅 2 种填充色（**白色背景 + 单色淡灰 panel**），核心组件用粗黑边框
  2. 模块名走 sans-serif bold；模块内的 sub-bullet 走 regular
  3. 箭头一律单向、实线，virtual 流量用 dashed；箭头粗细 1-1.5 pt（不是 5 pt 大粗箭头）
  4. 分层用淡灰 rectangle 圈起来（layer label 写左上角小字）
  5. 不出现渐变、阴影、3D 效果

**论文样例 B：Aegaeon（SOSP '25 Alibaba）Fig 1 + Fig 4 架构图**
- URL: <https://ennanzhai.github.io/pub/sosp25-aegaeon.pdf> / <https://dl.acm.org/doi/10.1145/3731569.3764815>
- 关键模式：
  1. 完全 2-3 色：白底 + 浅蓝（normal 组件）+ 浅黄（高亮组件 / contribution 部分）
  2. 数据流用编号气泡（① ② ③）替代长 caption——读者眼睛跟序号走
  3. 跨进程边界用 dashed 矩形围起（"Process A" / "Process B"）
  4. 模块内最多 1 行 tagline，多了走外部 caption

**论文样例 C：Alibaba HPN（SIGCOMM '24）Fig 3 物理拓扑**
- URL: <https://conferences.sigcomm.org/sigcomm/2024/accepted-papers/>
- 关键模式：
  1. 大量 box+连线时统一用**网格对齐**，所有 box 同尺寸
  2. 不同 tier 用「同色不同明度」（深蓝 → 浅蓝）而非「不同色相」
  3. 文字水平向（绝不竖排）

**我们应当复用的元素**（应用到 fuxi 8 张 drawio 架构图）：
- 调色板砍到 3 色：白底 / 浅灰 panel / 浅蓝高亮，不再是 v2 的彩虹分层
- 跨进程/跨节点一律 dashed 大矩形圈起
- 数据流用编号气泡 + caption 解释，少在线条中间塞文字

---

### 2.2 吞吐扩展性曲线（throughput vs N workers / vs concurrency）

**论文样例 A：Aegaeon Fig 13/14 throughput-vs-arrival-rate**
- URL: <https://ennanzhai.github.io/pub/sosp25-aegaeon.pdf>
- 关键模式：
  1. **不用 dual y-axis**——scaling efficiency 单独一张子图（subplots(1,2)），不同轴别叠
  2. baseline 用灰色虚线 + 圆点；contribution 系统用粗实线 + 方块（让自家系统跳出来）
  3. x 轴是 log2 scale 时（worker 数 1/2/4/8/16），ticks 标 1, 2, 4, 8, 16，**不**标 10⁰ 10¹
  4. 理论上限用细虚线 + 浅灰，**不抢主角**
  5. legend 放图内右下/左上空白处，不放图外（图外 legend 浪费纵向空间）

**论文样例 B：CASSINI Fig 11/12 多 job 收益曲线**
- URL: <https://www.usenix.org/system/files/nsdi24-rajasekaran.pdf>
- 关键模式：
  1. 自家系统用**最饱和颜色**（深蓝），所有 baseline 用浅灰/浅橙
  2. error bar / shaded region 用 alpha=0.15 同色填充，**不**用大 T-bar
  3. 95% CI 写在 caption 里，不在图上画

**我们应当复用的元素**（fig-5-1 scalability 必改）：
- 删掉 dual y-axis：左图 throughput vs N，右图 efficiency vs N
- 自家曲线深色 + 实线 + 圆点；理论上限改成浅灰细虚线
- x 轴 1/2/4/8/16 用 `xscale='log', base=2` + 显式 ticks

---

### 2.3 延迟分布图（CDF / violin / box-and-whisker）

**论文样例 A：systems 圈金标准是 CDF（不是 violin）**
- 多数 SOSP/OSDI/NSDI 论文用 **CDF**，violin/box 只在 ML systems 偶现
- 关键模式：
  1. y 轴永远 0-1（或 0%-100%），label 写 "CDF" 或 "Cumulative fraction"
  2. x 轴 latency log scale（重要！tail 才看得见）
  3. p50 / p99 用**水平虚线** annotate，不是图例
  4. 多 system 比对时，自家系统粗实线 + 饱和色，baseline 细线 + 浅色
  5. tail 重要时画 **CCDF**（complementary CDF = 1 - CDF）于 log-y，让 p99/p999 在视觉上拉开

**论文样例 B：danluu / Marc Brooker 反复倡导的 eCDF over histogram**
- URL: <https://brooker.co.za/blog/2022/09/02/ecdf.html>, <https://danluu.com/latency-pitfalls/>
- 关键论点：
  1. histogram 选 bin 数会作弊，eCDF 不需要 bin 选择
  2. tail 看 CCDF + log-y，不要看 CDF + linear-y
  3. 把 mean / p50 / p99 标线画在 CDF 上比写在表里更直观

**我们应当复用的元素**（fig-5-4 dispatch latency 大改）：
- 把 4 根「min/p50/p99/max」彩色柱**整张废掉**——这是反公理：bar plot 表分布是教科书反面教材
- 改成 CDF（n=500 的 5-run 数据足够画 eCDF），x=ms log-scale，y=cumulative fraction
- 在 p50 / p99 处加 dashed annotate
- 颜色一根线就够，配色用单色深蓝

---

### 2.4 延迟 breakdown（横向堆叠柱 / sankey）

**论文样例 A：Aegaeon Fig 12 latency breakdown**
- URL: <https://ennanzhai.github.io/pub/sosp25-aegaeon.pdf>
- 关键模式：
  1. **stacked bar 永远在 linear x 轴**（log + stacked = 视觉骗子，宽度不可加）
  2. 每个 stack 段直接在段中心写数值（"32 ms"），无需 legend 来回查
  3. 颜色按 Okabe-Ito：浅蓝 / 浅橙 / 浅绿 / 灰；4 段以内不需要太花
  4. 总长度也写在最右端外侧 "= 87 ms"

**论文样例 B：DistServe / SplitWise 系列 prefill-decode breakdown**
- 关键模式：
  1. 横向多组 bar（不同 workload）共享同一 legend，组间留 0.4 bar 间距
  2. Sankey 只在「端到端流经多个组件」叙事强时才用；breakdown 用 stacked bar 即可

**我们应当复用的元素**（fig-5-6 e2e-breakdown 必改）：
- **去掉 log x 轴**——log + stacked 是 v2 的硬伤，10ms 任务那一根的「L_disp 比 L_exec 还宽」是视觉假象
- 改 linear x；如果 1000ms / 100ms / 10ms 跨 2 个数量级实在容不下，**拆 3 张子图各自 linear**（三联图）
- 在每一段堆叠中心直接写数值，比 legend 更直观
- 调色板换 Okabe-Ito：`#56B4E9 / #E69F00 / #009E73 / #CC79A7`

---

### 2.5 消融实验图（grouped bar / heatmap）

**论文样例 A：grouped bar 主流**
- 关键模式：
  1. group 内 3-5 根柱，组间 0.4 间距，组内 0 间距
  2. 「baseline → +featureA → +featureB → full」用单色不同明度（不用色相变化）
  3. 数值标在柱顶 (`ax.bar_label`)，无小数位（throughput 取整）
  4. y 轴 0 起点 强制（baseline 比较时不能截断）

**论文样例 B：heatmap 用法**
- 仅在 2D ablation 时用（如「context window × precision」）
- colormap 选 sequential（viridis / Blues），**禁用** rainbow/jet
- 单元格里直接写数值，颜色只是辅助导航

**我们应当复用的元素**：
- v2 暂无 ablation 图；v3 如果加（task #11 用户曾提到），按 grouped bar + 单色明度梯度

---

### 2.6 状态机/时序图（sequence / FSM）

**论文样例 A：Aegaeon Fig 6 token-level scheduling 时序**
- 关键模式：
  1. 时序图（sequence diagram）：lifeline 黑色细线，激活段灰底色块；消息箭头实线
  2. 关键事件加红色短 tag（"⟵ key insight"），其余黑色
  3. **不**用 PlantUML 默认黄色块——太花，自己画 tikz/drawio

**论文样例 B：FSM 状态机**
- 关键模式：
  1. 圆角矩形表 state，箭头标转移条件 / 动作
  2. initial state 用粗边框；terminal state 双线边框
  3. self-loop 不要画到压住别的元素

**我们应当复用的元素**（fig-3-2 task-lifecycle）：
- v2 用了 graphviz/dot，结果是默认椭圆 + 自动布局，状态名挤一起——**手 layout 一遍 drawio**
- 转移条件标在箭头中点上方，配合细虚线辅助

---

## 3. 工具推荐

### 3.1 matplotlib rcParams 推荐配置（替换 v2 style.py）

```python
import matplotlib as mpl
from cycler import cycler

# Okabe-Ito 8 色（Wong palette），Nature 标准 colorblind-safe
OKABE_ITO = [
    "#0072B2",  # blue       —— 自家系统主色
    "#E69F00",  # orange     —— 第一 baseline
    "#009E73",  # bluish green
    "#CC79A7",  # reddish purple
    "#56B4E9",  # sky blue
    "#D55E00",  # vermillion
    "#F0E442",  # yellow（少用，对比度低）
    "#000000",  # black     —— 理论上限/虚线参考
]

ONE_MM = 1 / 25.4
SINGLE = 85 * ONE_MM         # USENIX/ACM 单列 = 3.35 in
DOUBLE = 178 * ONE_MM        # 双栏跨页 = 7.0 in
GOLDEN = 1.618

def apply():
    mpl.rcParams.update({
        # 尺寸与导出
        "figure.figsize": (SINGLE, SINGLE / GOLDEN),
        "figure.dpi": 150,                # 屏幕预览
        "savefig.dpi": 600,               # 备份 PNG 用
        "savefig.format": "pdf",          # 默认存 PDF
        "savefig.bbox": "tight",
        "savefig.pad_inches": 0.02,
        "pdf.fonttype": 42,               # TrueType embed，不出现 type-3 警告
        "ps.fonttype": 42,

        # 字号（注意：英文 8pt = 论文正文 ~9-10pt，缩放后看着合适）
        "font.size": 8,
        "axes.titlesize": 8,
        "axes.labelsize": 8,
        "xtick.labelsize": 7,
        "ytick.labelsize": 7,
        "legend.fontsize": 7,

        # 字体（正文 + CJK fallback）
        "font.family": "sans-serif",
        "font.sans-serif": ["Helvetica", "Arial", "PingFang SC",
                            "Heiti SC", "DejaVu Sans"],
        "axes.unicode_minus": False,

        # 线条与轴
        "axes.linewidth": 0.6,
        "lines.linewidth": 1.2,
        "lines.markersize": 4,
        "xtick.major.width": 0.5,
        "ytick.major.width": 0.5,
        "xtick.direction": "in",
        "ytick.direction": "in",
        "xtick.minor.visible": True,
        "ytick.minor.visible": True,

        # spine（隐藏 top/right，Tufte 风）
        "axes.spines.top": False,
        "axes.spines.right": False,

        # grid（默认关；要画的图自己开）
        "axes.grid": False,
        "grid.linewidth": 0.4,
        "grid.alpha": 0.25,
        "grid.color": "0.85",

        # legend
        "legend.frameon": False,
        "legend.handlelength": 1.5,
        "legend.borderaxespad": 0.4,

        # 调色板
        "axes.prop_cycle": cycler(color=OKABE_ITO),
    })
```

### 3.2 加分包

| 工具 | 用途 | 是否推荐 |
|------|------|----------|
| **scienceplots** | `plt.style.use(['science', 'ieee'])` 一行变顶刊样 | ✅ 推荐，配合 `nature` 或 `ieee` style 二选一 |
| **tueplots** | TUE 出的 NeurIPS / ICLR 模板，自动选字号 | ⚠️ 偏 ML，systems 圈用得少 |
| **palettable** | 各种科学调色板（Brewer / Wesanderson / cmocean） | ✅ 备选连续 colormap |
| **proplot** | 上层封装，sane defaults | ⚠️ 学习成本高，看完调研对个人项目不值 |
| **adjustText** | 自动避免 label 重叠 | ✅ 多 label 散点图必装 |

### 3.3 vector format 必须

- LaTeX 直接 `\includegraphics[width=\linewidth]{fig.pdf}`
- 如果用 png，**最低 600 dpi**，否则评委 PDF 阅读器放大就糊
- 决定性原则：**评委不会重启字体放大镜来看你的图**

### 3.4 Inkscape 后期微调

很常见的工作流：matplotlib 出 90% → Inkscape 加 callout / arrow / 编号气泡 → 存 PDF。
- macOS 装：`brew install --cask inkscape`
- 关键：matplotlib 存 PDF 时 `pdf.fonttype=42`，进 Inkscape 才是可编辑文字而不是 path

---

## 4. 立刻可应用到 fuxi 论文的 5 个改进项（按优先级）

### P0 · 替换 style.py 调色板 + 出 PDF（30 分钟工）
- 把 `tab10` 子集换成 Okabe-Ito 8 色
- `savefig.format='pdf'`、`pdf.fonttype=42`
- 全图重出，**所有 6 张实验图直接受益**

### P1 · 删掉所有图内 title（10 分钟）
- v2 现在 `任务派发延迟分布 (n=500, 5-run)` 这种文字必须搬到 LaTeX caption
- 顶刊 reviewer 看到图内 title 第一反应「博客作者」

### P2 · fig-5-4 从 bar→CDF 重构（2 小时）
- 当前 4 根彩色柱（min/p50/p99/max）是表数据用图像化，等于浪费空间
- n=500 × 5-run = 2500 样本，足够画 eCDF
- 版面节省一半，信息量翻倍

### P3 · fig-5-6 拆 log+stacked 反模式（1 小时）
- 拆成 1×3 subplots（10ms / 100ms / 1000ms 各自 linear x）
- 或者改 grouped bar（component 是组，task 是 group），linear y
- 把 L_enq / L_evt 数值化展示（v2 现在小到看不见）

### P4 · 8 张架构图统一调色（半天）
- 当前 v2 drawio 是淡蓝 / 淡黄 / 淡绿 / 淡紫 4 色 panel —— 太花
- 改 3 色：白底 / 浅灰（普通模块）/ 浅蓝（contribution / 玄女核心组件）
- 所有 cross-process 边界 dashed 矩形圈起，cross-node 双线
- 数据流加编号气泡（① ② ③）

---

## 5. Anti-pattern（不要做的）

1. **图内塞 title**——caption 的活，全删
2. **dual y-axis**（v2 fig-5-1 犯了）——双轴在 systems 圈被视为误导工具，拆 subplots
3. **log-x + stacked bar**（v2 fig-5-6 犯了）——log 让宽度不再可加，stack 含义崩
4. **彩虹/jet colormap**——感知不均匀 + colorblind 灾难，永远 viridis / Blues / Okabe-Ito
5. **bar 表分布**（v2 fig-5-4 犯了）——分布用 CDF / violin / box，bar 永远是表均值/总量
6. **图例放图外占大块版面**——能放图内空白处一定放图内
7. **3D bar / 3D pie**——AI 绘图常见雷，systems 圈零容忍
8. **饱和度拉满的红绿同框**——8% 男性 colorblind 看你图就是噪声
9. **300 dpi PNG 当成 PDF 替代品**——LaTeX `\includegraphics` 路径吃 PDF/SVG 不掉精度
10. **graphviz/dot 自动 layout 当架构图**——v2 fig-3-* 是这个坑，状态名挤一起；手画 drawio 永远赢
11. **架构图 emoji / icon 图标**——venue submission 风格冲突，删干净
12. **每根线一个颜色还都用主色**——同图 ≥4 系列时，自家系统饱和色 + baseline 全浅灰差明度

---

## 6. 一句话推荐：最值得参考的 1 篇

**Aegaeon (SOSP '25, Alibaba)**：<https://ennanzhai.github.io/pub/sosp25-aegaeon.pdf>

理由：
- 跟 fuxi 同样是「multi-component LLM serving / agent 编排平台」，figure 叙事路径几乎可复制
- Fig 1（架构）、Fig 6（时序）、Fig 12（breakdown）、Fig 13/14（throughput）四张正好覆盖我们 6 张实验图 + 8 张架构图的 80% 模式
- 中文一作组 + Alibaba production 系统，最贴近毕设语境

次推荐：**CASSINI (NSDI '24, MIT)** <https://www.usenix.org/system/files/nsdi24-rajasekaran.pdf>——CDF / 多 baseline 对比 / 分层架构图都是教科书级。

---

## 7. v2 当前 14 张图各自最大改进点（一句话/图）

### 实验图（6 张 matplotlib）

| 图 | 当前问题 | 一句话改 |
|---|---|---|
| fig-5-1 scalability | dual y-axis + 理论上限抢戏 | 拆 1×2 subplots：左 throughput-vs-N，右 efficiency-vs-N，理论上限改浅灰细虚线 |
| fig-5-2 poll-scan | （未展开看）猜测同样 dual-axis 风险 | 只画 fuxi vs poll baseline 两条线，理论值丢 caption 里；x 用 log 但 ticks 显式标 |
| fig-5-3 bus-stress | 应该是吞吐 vs 并发 | 自家系统深蓝实线，baseline 浅灰，shaded region 用 alpha=0.15 表 95% CI |
| fig-5-4 dispatch-latency | bar 表分布是反模式 | **整张废掉**，改 eCDF（log-x latency, linear-y CDF），p50/p99 dashed 标线 |
| fig-5-5 event-flow-latency | （未展开看）大概率同 5-4 问题 | 同上，eCDF 重画；如果是按 stage 比对，改 violin |
| fig-5-6 e2e-breakdown | log-x + stacked bar 双坑 | 拆 1×3 linear-x subplots，或改 grouped bar；段中心写数值 |

### 架构图（8 张 drawio）

| 图 | 当前问题 | 一句话改 |
|---|---|---|
| fig-2-1 system-architecture | 4 色淡彩 panel + 模块小字密 | 砍到 2-3 色（白/浅灰/浅蓝），高亮玄女层；模块 tagline 删多余字 |
| fig-3-1 module-dependency | 大概率 graphviz 自动 layout | 手 layout drawio，按 crate 依赖深度纵向分层 |
| fig-3-2 task-lifecycle | dot 椭圆挤一起 | 改 drawio 圆角矩形 FSM，转移条件标箭头中点 |
| fig-3-3 eventbus-topology | 可能是发布订阅 fan-out | 中心节点（EventBus）粗边框 + 浅蓝填充，订阅者一律白底；线细 1pt |
| fig-3-4 workspace-isolation | 三层隔离的边界容易模糊 | 三层 nested dashed 矩形（process / git worktree / fs），层 label 写左上角小字 |
| fig-3-5 collaboration-sequence | 时序图最容易花哨 | lifeline 黑细线，激活段浅灰填充；玄女 lifeline 加深蓝边框区分 |
| fig-4-1 dist-task-flow | 分布式数据流容易拉一坨 | 编号气泡 ① → ⑤ 表执行顺序，跨节点 dashed 矩形圈出 host A / host B |
| fig-4-2 agent-process-boundary | 边界图核心是「内/外」对比 | 自家进程深蓝色块，外部 cc/codex 进程白底虚线框，箭头标 stdin/stdout/A2A |

---

## 8. Sources

- [Tips for Creating Academic Figures with Matplotlib — AC Dustbin](https://allanchain.github.io/blog/post/mpl-paper-tips/)
- [Luís Cruz · 16 Guidelines for Effective Data Visualizations in Academic Papers](https://luiscruz.github.io/2021/03/01/effective-visualizations.html)
- [Bastian Bloessl · Publication-Quality Plots with Matplotlib](https://www.bastibl.net/publication-quality-plots/)
- [jbmouret/matplotlib_for_papers (GitHub)](https://github.com/jbmouret/matplotlib_for_papers)
- [Okabe-Ito Color Palette: 8 Hex Codes Reference](https://conceptviz.app/blog/okabe-ito-palette-hex-codes-complete-reference)
- [Marc Brooker · Histogram vs eCDF](https://brooker.co.za/blog/2022/09/02/ecdf.html)
- [Dan Luu · Some latency measurement pitfalls](https://danluu.com/latency-pitfalls/)
- [SciencePlots (matplotlib style)](https://github.com/garrettj403/SciencePlots)
- [matplotlib · Cumulative distributions example](https://matplotlib.org/stable/gallery/statistics/histogram_cumulative.html)
- [matplotlib · Box plot vs. violin plot comparison](https://matplotlib.org/stable/gallery/statistics/boxplot_vs_violin.html)
- [Tufte's Principles of Data-Ink (community notes)](https://jtr13.github.io/cc19/tuftes-principles-of-data-ink.html)
- [CASSINI · NSDI '24 paper PDF](https://www.usenix.org/system/files/nsdi24-rajasekaran.pdf)
- [Aegaeon · SOSP '25 paper PDF](https://ennanzhai.github.io/pub/sosp25-aegaeon.pdf)
- [SIGCOMM 2024 Accepted Papers](https://conferences.sigcomm.org/sigcomm/2024/accepted-papers/)
- [OSDI '24 Technical Sessions (USENIX)](https://www.usenix.org/conference/osdi24/technical-sessions)
- [SOSP 2024 dblp index](https://dblp.org/rec/conf/sosp/2024.html)
