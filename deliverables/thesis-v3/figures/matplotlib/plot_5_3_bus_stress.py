"""图 5-3 v3：事件总线 publish_tps & p99 vs N subscribers，按 rate 分面。

按 figure-design-conventions.md §2.2：
- 自家深蓝实线 + marker，浅色区分 payload size
- 拐点（64 sub × 100k/s）单独高亮
- subplots(1,2)：左 publish_tps（log y）、右 recv_p99_us（log y），共享 x（subscribers log2）
"""
import sys
from pathlib import Path
from collections import defaultdict

sys.path.insert(0, str(Path(__file__).parent.parent))
from style import apply, OKABE_ITO, DOUBLE, GOLDEN  # noqa: E402

import matplotlib.pyplot as plt  # noqa: E402

sys.path.insert(0, str(Path(__file__).parent))
from _data import parse_bus_stress  # noqa: E402

apply()

OUT = Path(__file__).parent / "fig-5-3-bus-stress.pdf"


def main():
    rows = parse_bus_stress()
    # group by (rate, payload) → list of (subs, tps, p99_us, drops)
    groups = defaultdict(list)
    for subs, rate, payload, tps, p50, p99, drops in rows:
        groups[(rate, payload)].append((subs, tps, p99, drops))

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(DOUBLE, DOUBLE / GOLDEN / 1.4))

    rate_styles = {
        1000: ("-o", OKABE_ITO[4]),       # sky blue 浅
        10000: ("-s", OKABE_ITO[1]),      # orange
        100000: ("-^", OKABE_ITO[0]),     # 主蓝（重点 rate）
    }
    # p99 < 1 μs 的样本受 OS 时钟分辨率（macOS SystemTime ns→us 截断后 1 μs）限制，
    # 在 log y 轴上无法表达。统一替换为 0.5 μs 绘图，并在图注 + 正文 §5.7 说明。
    P99_FLOOR = 0.5
    for (rate, payload), pts in sorted(groups.items()):
        if payload != "small":
            continue  # large 与 small 数据近乎重合，只画 small 简化图
        pts.sort()
        xs = [p[0] for p in pts]
        ys_tps = [p[1] for p in pts]
        ys_p99 = [max(p[2], P99_FLOOR) for p in pts]
        ys_drops = [p[3] for p in pts]
        marker, color = rate_styles[rate]
        label = f"rate = {rate // 1000} k ev/s"
        ax1.plot(xs, ys_tps, marker, color=color, label=label, linewidth=1.5, markersize=4.5)
        ax2.plot(xs, ys_p99, marker, color=color, label=label, linewidth=1.5, markersize=4.5)
        # 标注拐点
        for x, p99, drops in zip(xs, ys_p99, ys_drops):
            if drops > 0:
                ax2.annotate(f"drops {drops/1e6:.1f} M", xy=(x, p99),
                             xytext=(-50, -14), textcoords="offset points",
                             fontsize=6, color=OKABE_ITO[5])
                ax2.scatter([x], [p99], s=40, facecolors="none",
                            edgecolors=OKABE_ITO[5], linewidth=1.0, zorder=4)

    for ax in (ax1, ax2):
        ax.set_xscale("log", base=2)
        ax.set_xticks([1, 4, 16, 64])
        ax.set_xticklabels(["1", "4", "16", "64"])
        ax.set_xlabel("Subscribers")
        ax.grid(True, axis="y", alpha=0.25)

    ax1.set_yscale("log")
    ax1.set_ylabel("Publish 吞吐 (events/s)")
    ax1.legend(loc="lower right")

    ax2.set_yscale("log")
    ax2.set_ylabel("Recv p99 (μs)")
    # 标 1 μs 时钟精度下界，让读者看清「绝大多数 p99 落在分辨率以下」
    ax2.axhline(1.0, color="#666666", linestyle=":", linewidth=0.8)
    ax2.text(1.02, 1.05, "OS 时钟下界 1 μs", color="#666666", fontsize=6,
             transform=ax2.get_yaxis_transform())
    ax2.legend(loc="upper left")

    fig.tight_layout()
    fig.savefig(OUT)
    fig.savefig(OUT.with_suffix(".png"), dpi=200)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
