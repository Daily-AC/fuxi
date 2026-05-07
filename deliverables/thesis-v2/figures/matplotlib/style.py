"""所有论文实验图共用的 matplotlib 样式。
A4 单栏宽度 ~6.5 inch / dpi 300 / 11pt 字 / viridis 色（色盲友好）。
用法：
    from style import apply, DISCRETE_PALETTE
    apply()
"""
import matplotlib as mpl


def apply():
    # CJK：macOS 用 PingFang SC / Heiti TC；fallback 到 Arial Unicode 避免方框
    mpl.rcParams.update({
        "figure.figsize": (6.5, 4.0),
        "figure.dpi": 300,
        "savefig.dpi": 300,
        "savefig.bbox": "tight",
        "font.size": 11,
        "font.sans-serif": ["PingFang SC", "Heiti TC", "Arial Unicode MS", "DejaVu Sans"],
        "font.family": "sans-serif",
        "axes.unicode_minus": False,
        "axes.titlesize": 12,
        "axes.labelsize": 11,
        "xtick.labelsize": 10,
        "ytick.labelsize": 10,
        "legend.fontsize": 10,
        "lines.linewidth": 1.6,
        "axes.grid": True,
        "grid.alpha": 0.3,
        "axes.spines.top": False,
        "axes.spines.right": False,
    })


# tab10 中色盲友好的子集
DISCRETE_PALETTE = [
    "#1f77b4",  # blue
    "#ff7f0e",  # orange
    "#2ca02c",  # green
    "#d62728",  # red
    "#9467bd",  # purple
    "#8c564b",  # brown
]
