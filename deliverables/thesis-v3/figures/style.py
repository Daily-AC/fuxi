"""thesis-v3 matplotlib 风格——按 figure-design-conventions.md §3.1 配置。
对标 SOSP/NSDI/SIGCOMM 系统论文图标准。

主要变化（vs v2）：
- Okabe-Ito 8 色调色板（Wong palette，colorblind-safe）
- 默认存 PDF（pdf.fonttype=42 TrueType embed）
- USENIX/ACM 单列宽度 3.35 in / 双栏 7.0 in 常量
- 字号缩到 7-8 pt（论文里缩放后约 9-10 pt）
- tick 朝内、隐藏 top/right spine、grid 默认关
- 图内不写 title（caption 走 LaTeX）
"""
import matplotlib as mpl
from cycler import cycler

# ── 调色板：Okabe-Ito 8 色（Nature 标准 colorblind-safe）─────────────────
OKABE_ITO = [
    "#0072B2",  # blue          —— 自家系统主色
    "#E69F00",  # orange        —— 第一 baseline
    "#009E73",  # bluish green
    "#CC79A7",  # reddish purple
    "#56B4E9",  # sky blue
    "#D55E00",  # vermillion
    "#F0E442",  # yellow（少用，对比度低）
    "#000000",  # black         —— 理论上限/虚线参考
]

# ── 尺寸常量（USENIX/ACM 双栏惯例）─────────────────────────────────────
ONE_MM = 1 / 25.4
SINGLE = 85 * ONE_MM       # 单列 = 3.346 in
DOUBLE = 178 * ONE_MM      # 跨双栏 = 7.008 in
GOLDEN = 1.618


def apply():
    mpl.rcParams.update({
        # ── 尺寸与导出 ──
        "figure.figsize": (SINGLE, SINGLE / GOLDEN),
        "figure.dpi": 150,                # 屏幕预览
        "savefig.dpi": 600,               # 备份 PNG
        "savefig.format": "pdf",          # 默认 PDF
        "savefig.bbox": "tight",
        "savefig.pad_inches": 0.02,
        "pdf.fonttype": 42,               # TrueType embed
        "ps.fonttype": 42,

        # ── 字号 ──
        "font.size": 8,
        "axes.titlesize": 8,
        "axes.labelsize": 8,
        "xtick.labelsize": 7,
        "ytick.labelsize": 7,
        "legend.fontsize": 7,

        # ── 字体 + CJK fallback ──
        "font.family": "sans-serif",
        "font.sans-serif": [
            "Helvetica",
            "Arial",
            "PingFang SC",
            "Heiti SC",
            "DejaVu Sans",
        ],
        "axes.unicode_minus": False,

        # ── 线条 + 轴 ──
        "axes.linewidth": 0.6,
        "lines.linewidth": 1.2,
        "lines.markersize": 4,
        "xtick.major.width": 0.5,
        "ytick.major.width": 0.5,
        "xtick.direction": "in",
        "ytick.direction": "in",
        "xtick.minor.visible": True,
        "ytick.minor.visible": True,

        # ── spine（Tufte 风：隐藏 top/right）──
        "axes.spines.top": False,
        "axes.spines.right": False,

        # ── grid（默认关；要画的图自己开 axis='y' + alpha=0.25）──
        "axes.grid": False,
        "grid.linewidth": 0.4,
        "grid.alpha": 0.25,
        "grid.color": "0.85",

        # ── legend（无 frame，省空间）──
        "legend.frameon": False,
        "legend.handlelength": 1.5,
        "legend.borderaxespad": 0.4,

        # ── 调色板 ──
        "axes.prop_cycle": cycler(color=OKABE_ITO),
    })


# 保持向后兼容（v2 plot 脚本若 import）
DISCRETE_PALETTE = OKABE_ITO
