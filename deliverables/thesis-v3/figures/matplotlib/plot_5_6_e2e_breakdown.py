"""图 5-6 v3：端到端延迟分解 1×3 linear-x 子图（取代 v2 log+stacked 反模式）。

按 figure-design-conventions.md §2.4：
- 拆 3 张子图（10 ms / 100 ms / 1000 ms 任务），各自 linear x
- stacked horizontal bar，4 段（L_enq / L_disp / L_exec / L_evt）
- 段中心直接写数值，总长写在最右

公式 L_e2e = L_enq + L_disp + L_exec + L_evt
- L_disp_total = task_dispatch p50（baseline 表）
- 文字交代假设 L_enq : L_disp ≈ 3 : 7 split
- L_exec = job_sleep（10/100/1000 ms）
- L_evt = event_flow p50
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
from style import apply, OKABE_ITO, DOUBLE, GOLDEN  # noqa: E402

import matplotlib.pyplot as plt  # noqa: E402

sys.path.insert(0, str(Path(__file__).parent))
from _data import parse_latency_quantiles  # noqa: E402

apply()

OUT = Path(__file__).parent / "fig-5-6-e2e-breakdown.pdf"


def main():
    quants = parse_latency_quantiles()
    l_disp_total = quants["task_dispatch"]["p50_ms"]
    l_evt = quants["event_flow"]["p50_ms"]
    l_enq = l_disp_total * 0.3
    l_disp = l_disp_total * 0.7

    workloads = [("10 ms 任务", 10.0), ("100 ms 任务", 100.0), ("1000 ms 任务", 1000.0)]

    seg_labels = ["L_enq", "L_disp", "L_exec", "L_evt"]
    seg_colors = [OKABE_ITO[4], OKABE_ITO[1], OKABE_ITO[2], OKABE_ITO[3]]

    fig, axes = plt.subplots(1, 3, figsize=(DOUBLE, DOUBLE / GOLDEN / 1.6))

    for ax, (title, l_exec) in zip(axes, workloads):
        segs = [l_enq, l_disp, l_exec, l_evt]
        total = sum(segs)
        bottom = 0.0
        for v, label, color in zip(segs, seg_labels, seg_colors):
            ax.barh([0], [v], left=[bottom], color=color, edgecolor="black",
                    linewidth=0.5, label=label)
            # 段中心写数值（仅当段宽度足够）
            if v / total > 0.05:
                ax.text(bottom + v / 2, 0, f"{v:.1f}",
                        ha="center", va="center", fontsize=6.5, color="black")
            bottom += v
        # 总长写最右外
        ax.text(total * 1.02, 0, f"= {total:.1f} ms", ha="left", va="center",
                fontsize=7, color="0.30")
        ax.set_xlim(0, total * 1.22)
        ax.set_yticks([])
        ax.set_xlabel("延迟 (ms)")
        ax.set_title(title, pad=4)
        ax.grid(True, axis="x", alpha=0.25)
        ax.spines["left"].set_visible(False)

    # 共享 legend 放最下
    handles, labels = axes[0].get_legend_handles_labels()
    fig.legend(handles, labels, ncol=4, loc="lower center", bbox_to_anchor=(0.5, -0.02),
               frameon=False, fontsize=7)

    fig.tight_layout(rect=(0, 0.04, 1, 1))
    fig.savefig(OUT)
    fig.savefig(OUT.with_suffix(".png"), dpi=200)
    print(f"saved {OUT}")


if __name__ == "__main__":
    main()
